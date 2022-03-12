use anchor_lang::prelude::*;
use anchor_spl::token::{self, TokenAccount, Token, Mint};
use anchor_lang::solana_program::{sysvar, clock, program_option::COption};

declare_id!("DuPw7Lsvkr9XM5H3nv8733eCznT7hBWYjCkb1UV9YYex");

#[program]
pub mod nft_collaterized_loans {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, nonce: u8) -> Result<()> {
        let nft_collaterized_loans = &mut ctx.accounts.nft_collaterized_loans;
        nft_collaterized_loans.stablecoin_mint = ctx.accounts.stablecoin_mint.key();
        nft_collaterized_loans.stablecoin_vault = ctx.accounts.stablecoin_vault.key();
        nft_collaterized_loans.order_id = 0;
        nft_collaterized_loans.total_additional_collateral = 0;
        nft_collaterized_loans.nonce = nonce;

        Ok(())
    }

    pub fn create_order(ctx: Context<CreateOrder>, nonce:u8, request_amount: u64, interest: u64, period: u64, additional_collateral: u64) -> Result<()> {
        if request_amount == 0 {
            return Err(ErrorCode::AmountMustBeGreaterThanZero.into());
        }

        // Transfer collateral to vault.
        {
            let cpi_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.user_nft_vault.to_account_info(),
                    to: ctx.accounts.nft_vault.to_account_info(),
                    authority: ctx.accounts.borrower.to_account_info(), //todo use user account as signer
                },
            );
            token::transfer(cpi_ctx, 1)?;
        }

        // Transfer additional collateral to vault
        {
            let cpi_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.user_stablecoin_vault.to_account_info(),
                    to: ctx.accounts.stablecoin_vault.to_account_info(),
                    authority: ctx.accounts.borrower.to_account_info(), //todo use user account as signer
                },
            );
            token::transfer(cpi_ctx, additional_collateral)?;
        }

        let clock = clock::Clock::get().unwrap();

        // Save Info
        let order = &mut ctx.accounts.order;
        order.borrower = ctx.accounts.borrower.key();
        order.stablecoin_vault = ctx.accounts.user_stablecoin_vault.key();
        order.nft_mint = ctx.accounts.nft_mint.key();
        order.nft_vault = ctx.accounts.nft_vault.key();
        order.request_amount = request_amount;
        order.interest = interest;
        order.period = period;
        order.additional_collateral = additional_collateral;
        order.lender = order.key(); // just a placeholder
        order.created_at = clock.unix_timestamp as u64;
        order.loan_start_time = 0; // placeholder
        order.paid_back_at = 0;
        order.withdrew_at = 0;
        order.nonce = nonce;

        let nft_collaterized_loans = &mut ctx.accounts.nft_collaterized_loans;
        nft_collaterized_loans.total_additional_collateral += additional_collateral;

        nft_collaterized_loans.order_id += 1;

        order.order_status = true;

        Ok(())
    }

    pub fn cancel_order(ctx: Context<CancelOrder>, order_id: u64) -> Result<()> {
        let order = &mut ctx.accounts.order;
        let nft_collaterized_loans = &mut ctx.accounts.nft_collaterized_loans;

        if order.loan_start_time != 0 && order.order_status == false {
            return Err(ErrorCode::LoanAlreadyStarted.into());
        }
        
        // Transfer back nft collateral.
        {
            let seeds = &[nft_collaterized_loans.to_account_info().key.as_ref(), &[nft_collaterized_loans.nonce]];
            let signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.nft_vault.to_account_info(),
                    to: ctx.accounts.user_nft_vault.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(), 
                },
                signer
            );
            token::transfer(cpi_ctx, 1)?;
        }

        // Transfer back additional collateral 
        {
            let seeds = &[nft_collaterized_loans.to_account_info().key.as_ref(), &[nft_collaterized_loans.nonce]];
            let signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.stablecoin_vault.to_account_info(),
                    to: ctx.accounts.user_stablecoin_vault.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(), 
                },
                signer
            );
            token::transfer(cpi_ctx, order.additional_collateral)?;
        }
        nft_collaterized_loans.total_additional_collateral -= order.additional_collateral;

        order.order_status = false;

        // Sidenote: Preferred to close the account after this

        Ok(())
    }

    pub fn give_loan(ctx: Context<GiveLoan>, order_id: u64) -> Result<()> {
        let order = &mut ctx.accounts.order;

        if order.loan_start_time != 0 && order.order_status == false {
            return Err(ErrorCode::LoanAlreadyStarted.into());
        }

        // Transfer back additional collateral 
        {
            let cpi_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.lender_stablecoin_vault.to_account_info(),
                    to: ctx.accounts.borrower_stablecoin_vault.to_account_info(),
                    authority: ctx.accounts.lender.to_account_info(), 
                },
            );
            token::transfer(cpi_ctx, order.request_amount)?;
        }

        // Save Info
        order.lender = ctx.accounts.lender.key();
        order.loan_start_time = clock::Clock::get().unwrap().unix_timestamp as u64;
        order.order_status = false;

        Ok(())
    }

    pub fn payback(ctx: Context<Payback>, order_id: u64) -> Result<()> {
        let order = &mut ctx.accounts.order;
        let nft_collaterized_loans = &mut ctx.accounts.nft_collaterized_loans;

        if order.loan_start_time == 0 && order.order_status == true {
            return Err(ErrorCode::LoanNotProvided.into());
        }

        let clock = clock::Clock::get().unwrap();
        if order.loan_start_time.checked_add(order.period).unwrap() < clock.unix_timestamp as u64 {
            return Err(ErrorCode::RepaymentPeriodExceeded.into());
        }
        
        // Save Info
        order.paid_back_at = clock.unix_timestamp as u64;

        // Pay Loan
        {
            let cpi_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.user_stablecoin_vault.to_account_info(),
                    to: ctx.accounts.lender_stablecoin_vault.to_account_info(),
                    authority: ctx.accounts.borrower.to_account_info(), 
                },
            );
            token::transfer(cpi_ctx, order.request_amount.checked_add(order.interest).unwrap())?;
        }

        // Transfer back nft collateral.
        {
            let seeds = &[nft_collaterized_loans.to_account_info().key.as_ref(), &[nft_collaterized_loans.nonce]];
            let signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.nft_vault.to_account_info(),
                    to: ctx.accounts.user_nft_vault.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(), 
                },
                signer
            );
            token::transfer(cpi_ctx, 1)?;
        }

        // Transfer back additional collateral 
        {
            let seeds = &[nft_collaterized_loans.to_account_info().key.as_ref(), &[nft_collaterized_loans.nonce]];
            let signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.stablecoin_vault.to_account_info(),
                    to: ctx.accounts.user_stablecoin_vault.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(), 
                },
                signer
            );
            token::transfer(cpi_ctx, order.additional_collateral)?;
        }
        nft_collaterized_loans.total_additional_collateral -= order.additional_collateral;

        // Sidenote: Preferred to close the account after this

        Ok(())
    }

    pub fn liquidate(ctx: Context<Liquidate>, order_id: u64) -> Result<()> {
        let order = &mut ctx.accounts.order;
        let nft_collaterized_loans = &mut ctx.accounts.nft_collaterized_loans;

        if order.loan_start_time == 0 && order.order_status == true {
            return Err(ErrorCode::LoanNotProvided.into());
        }

        let clock = clock::Clock::get().unwrap();
        if order.loan_start_time.checked_add(order.period).unwrap() > clock.unix_timestamp as u64 {
            return Err(ErrorCode::RepaymentPeriodNotExceeded.into());
        }
        
        if order.withdrew_at != 0 {
            return Err(ErrorCode::AlreadyLiquidated.into());
        }

        // Save Info
        order.withdrew_at = clock.unix_timestamp as u64;

        // Transfer nft collateral.
        {
            let seeds = &[nft_collaterized_loans.to_account_info().key.as_ref(), &[nft_collaterized_loans.nonce]];
            let signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.nft_vault.to_account_info(),
                    to: ctx.accounts.user_nft_vault.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(), 
                },
                signer
            );
            token::transfer(cpi_ctx, 1)?;
        }

        // Transfer additional collateral 
        {
            let seeds = &[nft_collaterized_loans.to_account_info().key.as_ref(), &[nft_collaterized_loans.nonce]];
            let signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.stablecoin_vault.to_account_info(),
                    to: ctx.accounts.lender_stablecoin_vault.to_account_info(),
                    authority: ctx.accounts.signer.to_account_info(), 
                },
                signer
            );
            token::transfer(cpi_ctx, order.additional_collateral)?;
        }
        nft_collaterized_loans.total_additional_collateral -= order.additional_collateral;

        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(nonce: u8)]
pub struct Initialize<'info> {
    #[account(
        zero
    )]
    pub nft_collaterized_loans: Box<Account<'info, NFTCollaterizedLoans>>,

    pub stablecoin_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = stablecoin_vault.owner == signer.key(),
    )]
    pub stablecoin_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        seeds = [
            nft_collaterized_loans.to_account_info().key.as_ref()
        ],
        bump = nonce,
    )]
    pub signer: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CreateOrder<'info> {
    #[account(
        mut,
        has_one = stablecoin_vault,
        has_one = stablecoin_mint
    )]
    pub nft_collaterized_loans: Box<Account<'info, NFTCollaterizedLoans>>,

    pub stablecoin_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = stablecoin_vault.owner == signer.key(),
    )]
    pub stablecoin_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = user_stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = user_stablecoin_vault.owner == borrower.key(),
    )]
    pub user_stablecoin_vault: Box<Account<'info, TokenAccount>>,
    
    #[account(
        mut,
        constraint = nft_mint.supply == 1,
        constraint = nft_mint.decimals == 0,
    )]
    pub nft_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = nft_vault.mint == nft_mint.key(),
        constraint = nft_vault.owner == signer.key(),
    )]
    pub nft_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = user_nft_vault.mint == nft_mint.key(),
        constraint = user_nft_vault.owner == borrower.key(),
    )]
    pub user_nft_vault: Box<Account<'info, TokenAccount>>,

    // Order.
    #[account(
        init_if_needed,
        payer = borrower,
        seeds = [
            nft_collaterized_loans.order_id.to_string().as_ref(),
            nft_collaterized_loans.to_account_info().key().as_ref()
        ],
        bump
    )]
    pub order: Box<Account<'info, Order>>,

    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        seeds = [
            nft_collaterized_loans.to_account_info().key.as_ref()
        ],
        bump = nft_collaterized_loans.nonce,
    )]
    pub signer: UncheckedAccount<'info>,

    // misc
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>
}

#[derive(Accounts)]
#[instruction(order_id: u64)]
pub struct CancelOrder<'info> {
    #[account(
        mut,
        has_one = stablecoin_vault,
        has_one = stablecoin_mint
    )]
    pub nft_collaterized_loans: Box<Account<'info, NFTCollaterizedLoans>>,

    // Order.
    #[account(
        mut,
        constraint = order.stablecoin_vault == stablecoin_vault.key(),
        constraint = order.borrower == borrower.key(),
        constraint = order.nft_vault == nft_vault.key(),
        constraint = order.nft_mint == nft_mint.key(),
        seeds = [
            order_id.to_string().as_ref(),
            nft_collaterized_loans.to_account_info().key().as_ref()
        ],
        bump = order.nonce
    )]
    pub order: Box<Account<'info, Order>>,

    pub stablecoin_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = stablecoin_vault.owner == signer.key(),
    )]
    pub stablecoin_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = user_stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = user_stablecoin_vault.owner == borrower.key(),
    )]
    pub user_stablecoin_vault: Box<Account<'info, TokenAccount>>,
    
    #[account(
        mut,
        constraint = nft_mint.supply == 1,
        constraint = nft_mint.decimals == 0,
    )]
    pub nft_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = nft_vault.mint == nft_mint.key(),
        constraint = nft_vault.owner == signer.key(),
    )]
    pub nft_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = user_nft_vault.mint == nft_mint.key(),
        constraint = user_nft_vault.owner == borrower.key(),
    )]
    pub user_nft_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        seeds = [
            nft_collaterized_loans.to_account_info().key.as_ref()
        ],
        bump = nft_collaterized_loans.nonce,
    )]
    pub signer: UncheckedAccount<'info>,

    // misc
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>
}

#[derive(Accounts)]
#[instruction(order_id: u64)]
pub struct GiveLoan<'info> {
    #[account(
        mut,
        has_one = stablecoin_vault,
        has_one = stablecoin_mint
    )]
    pub nft_collaterized_loans: Box<Account<'info, NFTCollaterizedLoans>>,

    // Order.
    #[account(
        mut,
        constraint = order.stablecoin_vault == stablecoin_vault.key(),
        constraint = order.borrower != lender.key(),
        seeds = [
            order_id.to_string().as_ref(),
            nft_collaterized_loans.to_account_info().key().as_ref()
        ],
        bump = order.nonce
    )]
    pub order: Box<Account<'info, Order>>,

    pub stablecoin_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = stablecoin_vault.owner == signer.key(),
    )]
    pub stablecoin_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = lender_stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = lender_stablecoin_vault.owner == lender.key(),
    )]
    pub lender_stablecoin_vault: Box<Account<'info, TokenAccount>>,
    #[account(
        constraint = borrower_stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = borrower_stablecoin_vault.owner == order.borrower,
    )]
    pub borrower_stablecoin_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub lender: Signer<'info>,

    #[account(
        seeds = [
            nft_collaterized_loans.to_account_info().key.as_ref()
        ],
        bump = nft_collaterized_loans.nonce,
    )]
    pub signer: UncheckedAccount<'info>,

    // misc
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>
}

#[derive(Accounts)]
#[instruction(order_id: u64)]
pub struct Payback<'info> {
    #[account(
        mut,
        has_one = stablecoin_vault,
        has_one = stablecoin_mint
    )]
    pub nft_collaterized_loans: Box<Account<'info, NFTCollaterizedLoans>>,

    // Order.
    #[account(
        mut,
        constraint = order.stablecoin_vault == stablecoin_vault.key(),
        constraint = order.borrower == borrower.key(),
        constraint = order.nft_vault == nft_vault.key(),
        constraint = order.nft_mint == nft_mint.key(),
        seeds = [
            order_id.to_string().as_ref(),
            nft_collaterized_loans.to_account_info().key().as_ref()
        ],
        bump = order.nonce
    )]
    pub order: Box<Account<'info, Order>>,

    pub stablecoin_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = stablecoin_vault.owner == signer.key(),
    )]
    pub stablecoin_vault: Box<Account<'info, TokenAccount>>,
    
    #[account(
        constraint = lender_stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = lender_stablecoin_vault.owner == order.lender,
    )]
    pub lender_stablecoin_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = user_stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = user_stablecoin_vault.owner == borrower.key(),
    )]
    pub user_stablecoin_vault: Box<Account<'info, TokenAccount>>,
    
    #[account(
        mut,
        constraint = nft_mint.supply == 1,
        constraint = nft_mint.decimals == 0,
    )]
    pub nft_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = nft_vault.mint == nft_mint.key(),
        constraint = nft_vault.owner == signer.key(),
    )]
    pub nft_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = user_nft_vault.mint == nft_mint.key(),
        constraint = user_nft_vault.owner == borrower.key(),
    )]
    pub user_nft_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub borrower: Signer<'info>,

    #[account(
        seeds = [
            nft_collaterized_loans.to_account_info().key.as_ref()
        ],
        bump = nft_collaterized_loans.nonce,
    )]
    pub signer: UncheckedAccount<'info>,

    // misc
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>
}

#[derive(Accounts)]
#[instruction(order_id: u64)]
pub struct Liquidate<'info> {
    #[account(
        mut,
        has_one = stablecoin_vault,
        has_one = stablecoin_mint
    )]
    pub nft_collaterized_loans: Box<Account<'info, NFTCollaterizedLoans>>,

    // Order.
    #[account(
        mut,
        constraint = order.stablecoin_vault == stablecoin_vault.key(),
        has_one = lender,
        constraint = order.nft_vault == nft_vault.key(),
        constraint = order.nft_mint == nft_mint.key(),
        seeds = [
            order_id.to_string().as_ref(),
            nft_collaterized_loans.to_account_info().key().as_ref()
        ],
        bump = order.nonce
    )]
    pub order: Box<Account<'info, Order>>,

    pub stablecoin_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = stablecoin_vault.owner == signer.key(),
    )]
    pub stablecoin_vault: Box<Account<'info, TokenAccount>>,
    
    #[account(
        constraint = lender_stablecoin_vault.mint == stablecoin_mint.key(),
        constraint = lender_stablecoin_vault.owner == lender.key(),
    )]
    pub lender_stablecoin_vault: Box<Account<'info, TokenAccount>>,
    
    #[account(
        mut,
        constraint = nft_mint.supply == 1,
        constraint = nft_mint.decimals == 0,
    )]
    pub nft_mint: Box<Account<'info, Mint>>,
    #[account(
        constraint = nft_vault.mint == nft_mint.key(),
        constraint = nft_vault.owner == signer.key(),
    )]
    pub nft_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        constraint = user_nft_vault.mint == nft_mint.key(),
        constraint = user_nft_vault.owner == lender.key(),
    )]
    pub user_nft_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub lender: Signer<'info>,

    #[account(
        seeds = [
            nft_collaterized_loans.to_account_info().key.as_ref()
        ],
        bump = nft_collaterized_loans.nonce,
    )]
    pub signer: UncheckedAccount<'info>,

    // misc
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>
}

#[account]
pub struct NFTCollaterizedLoans {
    // Mint of the token
    pub stablecoin_mint: Pubkey,
    // Vault holding the stablecoins -- mostly for holding the collateral stablecoins
    pub stablecoin_vault: Pubkey,
    // latest order id
    pub order_id: u64,
    // total additional collateral
    pub total_additional_collateral: u64,

    // nonce 
    pub nonce: u8
}

#[account]
#[derive(Default)]
pub struct Order {
    // person requesting the loan
    pub borrower: Pubkey,
    /// vault to send the loan 
    pub stablecoin_vault: Pubkey,
    // mint of the nft
    pub nft_mint: Pubkey,
    /// collateral vault holding the nft
    pub nft_vault: Pubkey,
    // request amount
    pub request_amount: u64,
    // interest amount
    pub interest: u64,
    // the loan period 
    pub period: u64,
    // additional collateral
    pub additional_collateral: u64,
    // lender
    pub lender: Pubkey,
    // order created at
    pub created_at: u64,
    // loan start time
    pub loan_start_time: u64,
    // repayment timestamp 
    pub paid_back_at: u64,
    // time the lender liquidated the loan & withdrew the collateral
    pub withdrew_at: u64,

    // status of the order
    pub order_status: bool,

    // nonce
    pub nonce: u8
}

#[error_code]
pub enum ErrorCode {
    #[msg("Amount must be greater than zero.")]
    AmountMustBeGreaterThanZero,
    #[msg("Loan has started or already been canceled")]
    LoanAlreadyStarted,
    #[msg("Loan not provided yet")]
    LoanNotProvided,
    #[msg("Repayment Period has been exceeded")]
    RepaymentPeriodExceeded,
    #[msg("Repayment Period has not been exceeded")]
    RepaymentPeriodNotExceeded,
    #[msg("Already liquidated")]
    AlreadyLiquidated,
}