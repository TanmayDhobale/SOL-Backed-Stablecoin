# SOL-Backed Stablecoin Smart Contract

## Overview

This Solana smart contract implements a stablecoin system backed by SOL (Solana's native token). Users can deposit SOL as collateral and mint stablecoins against it. The system maintains a specific collateralization ratio to ensure stability.

## Key Features

1. **Collateral Deposit**: Users can deposit SOL as collateral.
2. **Stablecoin Minting**: Users can mint stablecoins against their deposited collateral.
3. **Debt Repayment**: Users can repay their debt by burning stablecoins.
4. **Collateral Withdrawal**: Users can withdraw their SOL if their position remains healthy.
5. **Liquidation**: Unhealthy positions can be liquidated by other users.

## How It Works

1. **Depositing Collateral**: 
   - Users send SOL to the contract's vault.
   - The contract records the deposited amount for each user.

2. **Minting Stablecoins**:
   - Users request to mint stablecoins.
   - The contract checks if the user's position will remain healthy after minting.
   - If healthy, the contract mints stablecoins to the user's token account.

3. **Repaying Debt**:
   - Users send stablecoins to be burned.
   - The contract reduces the user's debt accordingly.

4. **Withdrawing Collateral**:
   - Users request to withdraw SOL.
   - The contract checks if the withdrawal keeps the position healthy.
   - If healthy, the contract sends SOL back to the user.

5. **Liquidation**:
   - Anyone can initiate liquidation of an unhealthy position.
   - The liquidator repays a portion of the debt and receives collateral at a discount.

## Key Components

- **Oracle**: Uses Pyth for real-time SOL price data.
- **Health Factor**: Calculates the safety of a user's position based on collateral value and debt.
- **Liquidation Threshold**: Sets the minimum health factor before a position can be liquidated.
- **Liquidation Bonus**: Incentivizes liquidators with extra collateral.

## Security Features

- Uses Program Derived Addresses (PDAs) for secure account management.
- Implements thorough error checking and handling.
- Uses checked math operations to prevent overflows.

