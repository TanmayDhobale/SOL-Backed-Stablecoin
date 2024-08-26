use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount};
use pyth_sdk_solana::load_price_feed_from_account_info;

declare_id!("Your_Program_ID_Here");

#[program]
pub mod sol_stablecoin {
    use super::*;

    pub fn initialize(
        ctx: Context<Initialize>,
        liquidation_threshold: u64,
        min_health_factor: u64,
        liquidation_bonus: u64,
    ) -> Result<()> {
        let state = &mut ctx.accounts.state;
        state.liquidation_threshold = liquidation_threshold;
        state.min_health_factor = min_health_factor;
        state.liquidation_bonus = liquidation_bonus;
        state.oracle = ctx.accounts.oracle.key();
        state.stablecoin_mint = ctx.accounts.stablecoin_mint.key();
        state.vault = ctx.accounts.vault.key();
        state.bump = *ctx.bumps.get("state").unwrap();
        Ok(())
    }

    pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
        // Transfer SOL from user to vault
        let cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.user.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
            },
        );
        anchor_lang::system_program::transfer(cpi_context, amount)?;

        // Update user's collateral balance
        ctx.accounts.user_account.collateral_amount = ctx.accounts.user_account.collateral_amount
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;

        emit!(CollateralDeposited {
            user: ctx.accounts.user.key(),
            amount,
        });

        Ok(())
    }

    pub fn mint_stablecoin(ctx: Context<MintStablecoin>, amount: u64) -> Result<()> {
        // Check health factor
        let health_factor = calculate_health_factor(&ctx.accounts.user_account, &ctx.accounts.state, amount, &ctx.accounts.oracle)?;
        require!(health_factor >= ctx.accounts.state.min_health_factor, ErrorCode::UnhealthyPosition);

        // Mint stablecoins
        let seeds = &[
            b"state".as_ref(),
            &[ctx.accounts.state.bump],
        ];
        let signer = &[&seeds[..]];
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            token::MintTo {
                mint: ctx.accounts.stablecoin_mint.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                authority: ctx.accounts.state.to_account_info(),
            },
            signer,
        );
        token::mint_to(cpi_ctx, amount)?;

        // Update user's debt
        ctx.accounts.user_account.debt_amount = ctx.accounts.user_account.debt_amount
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;

        emit!(StablecoinMinted {
            user: ctx.accounts.user.key(),
            amount,
        });

        Ok(())
    }

    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        // Burn stablecoins
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            token::Burn {
                mint: ctx.accounts.stablecoin_mint.to_account_info(),
                from: ctx.accounts.user_token_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::burn(cpi_ctx, amount)?;

        // Update user's debt
        ctx.accounts.user_account.debt_amount = ctx.accounts.user_account.debt_amount
            .checked_sub(amount)
            .ok_or(ErrorCode::MathOverflow)?;

        emit!(DebtRepaid {
            user: ctx.accounts.user.key(),
            amount,
        });

        Ok(())
    }

    pub fn withdraw_collateral(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()> {
        // Check if withdrawal would leave position healthy
        let new_collateral = ctx.accounts.user_account.collateral_amount
            .checked_sub(amount)
            .ok_or(ErrorCode::InsufficientCollateral)?;
        let health_factor = calculate_health_factor_with_collateral(
            &ctx.accounts.user_account,
            &ctx.accounts.state,
            new_collateral,
            &ctx.accounts.oracle
        )?;
        require!(health_factor >= ctx.accounts.state.min_health_factor, ErrorCode::UnhealthyPosition);

        // Transfer SOL from vault to user
        let seeds = &[
            b"state".as_ref(),
            &[ctx.accounts.state.bump],
        ];
        let signer = &[&seeds[..]];
        let cpi_context = CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.user.to_account_info(),
            },
            signer,
        );
        anchor_lang::system_program::transfer(cpi_context, amount)?;

        // Update user's collateral balance
        ctx.accounts.user_account.collateral_amount = new_collateral;

        emit!(CollateralWithdrawn {
            user: ctx.accounts.user.key(),
            amount,
        });

        Ok(())
    }

    pub fn liquidate(ctx: Context<Liquidate>, repay_amount: u64) -> Result<()> {
        // Check if position is unhealthy
        let health_factor = calculate_health_factor(&ctx.accounts.user_account, &ctx.accounts.state, 0, &ctx.accounts.oracle)?;
        require!(health_factor < ctx.accounts.state.min_health_factor, ErrorCode::PositionNotLiquidatable);

        // Calculate collateral to seize
        let collateral_price = get_sol_price(&ctx.accounts.oracle)?;
        let collateral_to_seize = repay_amount
            .checked_mul(10000)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(collateral_price)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_mul(ctx.accounts.state.liquidation_bonus)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;

        // Burn repaid stablecoins
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            token::Burn {
                mint: ctx.accounts.stablecoin_mint.to_account_info(),
                from: ctx.accounts.liquidator_token_account.to_account_info(),
                authority: ctx.accounts.liquidator.to_account_info(),
            },
        );
        token::burn(cpi_ctx, repay_amount)?;

        // Update user's debt and collateral
        ctx.accounts.user_account.debt_amount = ctx.accounts.user_account.debt_amount
            .checked_sub(repay_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        ctx.accounts.user_account.collateral_amount = ctx.accounts.user_account.collateral_amount
            .checked_sub(collateral_to_seize)
            .ok_or(ErrorCode::InsufficientCollateral)?;

        // Transfer seized collateral to liquidator
        let seeds = &[
            b"state".as_ref(),
            &[ctx.accounts.state.bump],
        ];
        let signer = &[&seeds[..]];
        let cpi_context = CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            anchor_lang::system_program::Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.liquidator.to_account_info(),
            },
            signer,
        );
        anchor_lang::system_program::transfer(cpi_context, collateral_to_seize)?;

        emit!(PositionLiquidated {
            user: ctx.accounts.user_account.key(),
            liquidator: ctx.accounts.liquidator.key(),
            repaid_amount: repay_amount,
            seized_collateral: collateral_to_seize,
        });

        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = user,
        space = 8 + 32 + 8 + 8 + 8 + 32 + 32 + 32 + 1
    )]
    pub state: Account<'info, State>,
    #[account(mut)]
    pub user: Signer<'info>,
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub oracle: AccountInfo<'info>,
    pub stablecoin_mint: Account<'info, Mint>,
    pub vault: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut,
        seeds = [b"vault"],
        bump,
    )]
    pub vault: SystemAccount<'info>,
    #[account(
        init_if_needed,
        payer = user,
        space = 8 + 8 + 8,
        seeds = [b"user", user.key().as_ref()],
        bump
    )]
    pub user_account: Account<'info, UserAccount>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct MintStablecoin<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub stablecoin_mint: Account<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = stablecoin_mint,
        associated_token::authority = user
    )]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        seeds = [b"user", user.key().as_ref()],
        bump
    )]
    pub user_account: Account<'info, UserAccount>,
    #[account(
        seeds = [b"state"],
        bump = state.bump
    )]
    pub state: Account<'info, State>,
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub oracle: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub stablecoin_mint: Account<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = stablecoin_mint,
        associated_token::authority = user
    )]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        seeds = [b"user", user.key().as_ref()],
        bump
    )]
    pub user_account: Account<'info, UserAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct WithdrawCollateral<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut,
        seeds = [b"vault"],
        bump,
    )]
    pub vault: SystemAccount<'info>,
    #[account(
        mut,
        seeds = [b"user", user.key().as_ref()],
        bump
    )]
    pub user_account: Account<'info, UserAccount>,
    #[account(
        seeds = [b"state"],
        bump = state.bump
    )]
    pub state: Account<'info, State>,
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub oracle: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Liquidate<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    #[account(mut)]
    pub user_account: Account<'info, UserAccount>,
    #[account(
        mut,
        seeds = [b"vault"],
        bump,
    )]
    pub vault: SystemAccount<'info>,
    #[account(mut)]
    pub stablecoin_mint: Account<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = stablecoin_mint,
        associated_token::authority = liquidator
    )]
    pub liquidator_token_account: Account<'info, TokenAccount>,
    #[account(
        seeds = [b"state"],
        bump = state.bump
    )]
    pub state: Account<'info, State>,
    /// CHECK: This is not dangerous because we don't read or write from this account
    pub oracle: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[account]
pub struct State {
    pub liquidation_threshold: u64,
    pub min_health_factor: u64,
    pub liquidation_bonus: u64,
    pub oracle: Pubkey,
    pub stablecoin_mint: Pubkey,
    pub vault: Pubkey,
    pub bump: u8,
}

#[account]
pub struct UserAccount {
    pub collateral_amount: u64,
    pub debt_amount: u64,
}

fn calculate_health_factor(user_account: &UserAccount, state: &State, new_debt: u64, oracle: &AccountInfo) -> Result<u64> {
    let total_debt = user_account.debt_amount.checked_add(new_debt).ok_or(ErrorCode::MathOverflow)?;
    if total_debt == 0 {
        return Ok(u64::MAX);
    }
    let collateral_value = get_collateral_value(user_account.collateral_amount, oracle)?;
    Ok(collateral_value
        .checked_mul(10000)
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(total_debt.checked_mul(state.liquidation_threshold).ok_or(ErrorCode::MathOverflow)?)
        .ok_or(ErrorCode::MathOverflow)?)
}

fn calculate_health_factor(
    user_account: &UserAccount,
    state: &State,
    new_debt: u64,
    oracle: &AccountInfo,
) -> Result<u64> {
    let price_feed = load_price_feed_from_account_info(oracle)?;
    let price = price_feed.get_current_price().ok_or(ErrorCode::OracleError)?;
    let total_debt = user_account.debt_amount.checked_add(new_debt).ok_or(ErrorCode::MathOverflow)?;
    
    if total_debt == 0 {
        return Ok(u64::MAX); // Max health factor if there's no debt
    }

    let collateral_value = user_account.collateral_amount
        .checked_mul(price)
        .ok_or(ErrorCode::MathOverflow)?;

    let health_factor = collateral_value
        .checked_mul(100)
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(total_debt)
        .ok_or(ErrorCode::MathOverflow)?;

    Ok(health_factor)
}

fn get_collateral_value(collateral_amount: u64, oracle: &AccountInfo) -> Result<u64> {
    let sol_price = get_sol_price(oracle)?;
    collateral_amount
        .checked_mul(sol_price)
        .ok_or(ErrorCode::MathOverflow)?
        .checked_div(10000)
        .ok_or(ErrorCode::MathOverflow)
}

fn get_sol_price(oracle: &AccountInfo) -> Result<u64> {
    let price_feed = load_price_feed_from_account_info(oracle).map_err(|_| ErrorCode::InvalidOracle)?;
    let current_price = price_feed.get_current_price().ok_or(ErrorCode::InvalidOracle)?;
    Ok(current_price.price)
}

#[error_code]
pub enum ErrorCode {
    #[msg("Math operation resulted in overflow")]
    MathOverflow,
    #[msg("User position is not healthy")]
    UnhealthyPosition,
    #[msg("Invalid oracle data")]
    InvalidOracle,
    #[msg("Insufficient collateral")]
    InsufficientCollateral,
    #[msg("Position is not liquidatable")]
    PositionNotLiquidatable,
}

#[event]
pub struct CollateralDeposited {
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct StablecoinMinted {
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct DebtRepaid {
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct CollateralWithdrawn {
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct PositionLiquidated {
    pub user: Pubkey,
    pub liquidator: Pubkey,
    pub repaid_amount: u64,
    pub seized_collateral: u64,
}
