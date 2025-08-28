//! Anchor 集成测试 + 纯函数单测
//! 运行：anchor test

use anchor_lang::prelude::*;
use anchor_lang::prelude::System;
use anchor_client::{
    anchor_lang::{InstructionData, ToAccountMetas},
    Client, Cluster, Program,
};
use anchor_spl::token::{self, Token, TokenAccount, Mint, ID as SPL_TOKEN_ID};
use solana_sdk::{
    signature::{Keypair, Signer},
    system_program,
    instruction::Instruction,
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
};
use spl_associated_token_account::get_associated_token_address;

use dg_solana_programs::{
    accounts as dg_accounts,
    instruction as dg_ix,
    OperationType, OperationData, TransferParams,
    apply_slippage_min, amounts_from_liquidity_burn_q64,
};


fn local_client() -> (Client, Keypair) {
    // 默认使用本地 validator（anchor test 会自动起）
    let payer = Keypair::new();
    let url = std::env::var("ANCHOR_PROVIDER_URL").unwrap_or_else(|_| "http://127.0.0.1:8899".into());
    let cluster = Cluster::Custom(url.clone(), url);
    let client = Client::new_with_options(cluster, RcSigner(payer.clone()), CommitmentConfig::processed());
    (client, payer)
}

#[derive(Clone)]
struct RcSigner(Keypair);
impl Signer for RcSigner {
    fn try_pubkey(&self) -> Result<Pubkey, solana_sdk::signer::SignerError> { Ok(self.0.pubkey()) }
    fn try_sign_message(&self, msg: &[u8]) -> Result<solana_sdk::signature::Signature, solana_sdk::signer::SignerError> {
        Ok(self.0.sign_message(msg))
    }
}

fn program(client: &Client) -> Program {
    // 这里的 ID 必须与你的 declare_id! 一致
    let prog_id = dg_solana_programs::id();
    client.program(prog_id)
}

async fn airdrop(program: &Program, kp: &Keypair, lamports: u64) {
    program
        .rpc()
        .request_airdrop(&kp.pubkey(), lamports)
        .unwrap();
    let _ = program.rpc().get_balance(&kp.pubkey()).unwrap();
}

async fn create_mint(program: &Program, authority: &Keypair, decimals: u8) -> Pubkey {
    let mint = Keypair::new();
    let rent = program.rpc().get_minimum_balance_for_rent_exemption(spl_token_2022::state::Mint::LEN).unwrap();

    // 创建 mint 账户
    let ix_create = solana_sdk::system_instruction::create_account(
        &authority.pubkey(),
        &mint.pubkey(),
        rent,
        spl_token_2022::state::Mint::LEN as u64,
        &SPL_TOKEN_ID,
    );
    // 初始化 mint
    let ix_init = spl_token::instruction::initialize_mint(
        &SPL_TOKEN_ID,
        &mint.pubkey(),
        &authority.pubkey(),
        None,
        decimals,
    ).unwrap();

    program
        .request()
        .instruction(ix_create)
        .instruction(ix_init)
        .signer(authority)
        .signer(&mint)
        .send()
        .unwrap();

    mint.pubkey()
}

async fn create_ata_and_mint_to(
    program: &Program,
    mint: Pubkey,
    owner: &Keypair,
    amount: u64,
) -> Pubkey {
    let ata = get_associated_token_address(&owner.pubkey(), &mint);
    let ix_create_ata = spl_associated_token_account::instruction::create_associated_token_account(
        &owner.pubkey(), &owner.pubkey(), &mint, &SPL_TOKEN_ID
    );
    let ix_mint_to = spl_token::instruction::mint_to(
        &SPL_TOKEN_ID, &mint, &ata, &owner.pubkey(), &[], amount
    ).unwrap();

    program
        .request()
        .instruction(ix_create_ata)
        .instruction(ix_mint_to)
        .signer(owner)
        .send()
        .unwrap();

    ata
}

/// 创建一个“任意 owner”的 TokenAccount（用于 program_token_account 归 PDA 所有）
async fn create_token_account_with_owner(
    program: &Program,
    mint: Pubkey,
    owner: Pubkey,
    funder: &Keypair,
) -> Pubkey {
    let ta = Keypair::new();
    let rent = program.rpc().get_minimum_balance_for_rent_exemption(spl_token_2022::state::Account::LEN).unwrap();

    let ix_create = solana_sdk::system_instruction::create_account(
        &funder.pubkey(),
        &ta.pubkey(),
        rent,
        spl_token_2022::state::Account::LEN as u64,
        &SPL_TOKEN_ID,
    );
    let ix_init = spl_token::instruction::initialize_account(
        &SPL_TOKEN_ID, &ta.pubkey(), &mint, &owner
    ).unwrap();

    program
        .request()
        .instruction(ix_create)
        .instruction(ix_init)
        .signer(funder)
        .signer(&ta)
        .send()
        .unwrap();

    ta.pubkey()
}

/// 计算本程序里 PDA: "operation_data"
fn operation_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"operation_data"], &dg_solana_programs::id())
}

/// 便捷 fetch
async fn fetch_operation(program: &Program, addr: Pubkey) -> OperationData {
    program.account::<OperationData>(addr).unwrap()
}

/// ----------------- Tests -----------------

#[tokio::test]
async fn initialize_ok() {
    let (client, payer) = local_client();
    let program = program(&client);
    airdrop(&program, &payer.0, 2_000_000_000).await;

    // derive PDA
    let (op_pda, _bump) = operation_pda();

    // 调用 initialize
    let ix = dg_ix::Initialize {};
    let accounts = dg_accounts::Initialize {
        operation_data: op_pda,
        authority: payer.0.pubkey(),
        system_program: system_program::id(),
    };
    program
        .request()
        .accounts(accounts)
        .args(ix)
        .signer(&payer.0)
        .send()
        .unwrap();

    let state = fetch_operation(&program, op_pda).await;
    assert!(state.initialized);
    assert_eq!(state.authority, payer.0.pubkey());
}

#[tokio::test]
async fn deposit_transfer_ok_and_state_set() {
    let (client, payer) = local_client();
    let program = program(&client);
    airdrop(&program, &payer.0, 2_000_000_000).await;

    // init
    let (op_pda, _bump) = operation_pda();
    program
        .request()
        .accounts(dg_accounts::Initialize {
            operation_data: op_pda,
            authority: payer.0.pubkey(),
            system_program: system_program::id(),
        })
        .args(dg_ix::Initialize {})
        .signer(&payer.0)
        .send()
        .unwrap();

    // 铸 1 个测试 mint & 给 authority 一些余额
    let mint = create_mint(&program, &payer.0, 6).await;
    let authority_ata = create_ata_and_mint_to(&program, mint, &payer.0, 1_000_000).await;

    // 创建“程序托管账户”——owner 必须是 op_pda
    let program_token_account = create_token_account_with_owner(&program, mint, op_pda, &payer.0).await;

    // 准备 TransferParams 参数，并序列化到 action
    let recipient = Keypair::new();
    let transfer_params = TransferParams {
        amount: 123_456,
        recipient: recipient.pubkey(),
    };
    let mut action = Vec::new();
    transfer_params.try_serialize(&mut action).unwrap();

    // 调用 deposit
    program
        .request()
        .accounts(dg_accounts::Deposit {
            operation_data: op_pda,
            authority: payer.0.pubkey(),
            authority_ata,
            program_token_account,
            token_program: SPL_TOKEN_ID,
        })
        .args(dg_ix::Deposit {
            transfer_id: "tx-abc".to_string(),
            operation_type: OperationType::Transfer,
            action: action.clone(),
            amount: transfer_params.amount,
        })
        .signer(&payer.0)
        .send()
        .unwrap();

    // 校验：PDA 状态被设置；余额已转入 program_token_account
    let after = fetch_operation(&program, op_pda).await;
    assert_eq!(after.transfer_id, "tx-abc");
    assert_eq!(after.amount, transfer_params.amount);
    assert_eq!(after.operation_type, OperationType::Transfer);
    assert_eq!(after.recipient, recipient.pubkey());
    assert!(!after.executed);

    // program_token_account 上应有 123456
    let ta: TokenAccount = program.account(program_token_account).unwrap();
    assert_eq!(ta.amount, transfer_params.amount);
}

#[tokio::test]
async fn deposit_rejects_invalid_amount_or_id() {
    let (client, payer) = local_client();
    let program = program(&client);
    airdrop(&program, &payer.0, 2_000_000_000).await;

    let (op_pda, _bump) = operation_pda();
    program
        .request()
        .accounts(dg_accounts::Initialize {
            operation_data: op_pda,
            authority: payer.0.pubkey(),
            system_program: system_program::id(),
        })
        .args(dg_ix::Initialize {})
        .signer(&payer.0)
        .send()
        .unwrap();

    // mint & accounts
    let mint = create_mint(&program, &payer.0, 0).await;
    let authority_ata = create_ata_and_mint_to(&program, mint, &payer.0, 10).await;
    let program_token_account = create_token_account_with_owner(&program, mint, op_pda, &payer.0).await;

    // amount=0 -> 应失败
    let res = program
        .request()
        .accounts(dg_accounts::Deposit {
            operation_data: op_pda,
            authority: payer.0.pubkey(),
            authority_ata,
            program_token_account,
            token_program: SPL_TOKEN_ID,
        })
        .args(dg_ix::Deposit {
            transfer_id: "ok".into(),
            operation_type: OperationType::Transfer,
            action: vec![],
            amount: 0,
        })
        .signer(&payer.0)
        .send();

    assert!(res.is_err(), "amount=0 应被拒绝");

    // transfer_id 为空 -> 应失败
    let res2 = program
        .request()
        .accounts(dg_accounts::Deposit {
            operation_data: op_pda,
            authority: payer.0.pubkey(),
            authority_ata,
            program_token_account,
            token_program: SPL_TOKEN_ID,
        })
        .args(dg_ix::Deposit {
            transfer_id: "".into(),
            operation_type: OperationType::Transfer,
            action: vec![],
            amount: 1,
        })
        .signer(&payer.0)
        .send();

    assert!(res2.is_err(), "空 transfer_id 应被拒绝");
}

#[tokio::test]
async fn modify_pda_authority_ok() {
    let (client, payer) = local_client();
    let program = program(&client);
    airdrop(&program, &payer.0, 2_000_000_000).await;

    let (op_pda, _bump) = operation_pda();
    program
        .request()
        .accounts(dg_accounts::Initialize {
            operation_data: op_pda,
            authority: payer.0.pubkey(),
            system_program: system_program::id(),
        })
        .args(dg_ix::Initialize {})
        .signer(&payer.0)
        .send()
        .unwrap();

    let new_auth = Keypair::new().pubkey();
    program
        .request()
        .accounts(dg_accounts::ModifyPdaAuthority {
            operation_data: op_pda,
            current_authority: payer.0.pubkey(),
        })
        .args(dg_ix::ModifyPdaAuthority { new_authority: new_auth })
        .signer(&payer.0)
        .send()
        .unwrap();

    let after = fetch_operation(&program, op_pda).await;
    assert_eq!(after.authority, new_auth);
}


#[test]
fn test_apply_slippage_min() {
    assert_eq!(apply_slippage_min(1000, 0), 1000);
    assert_eq!(apply_slippage_min(1000, 100), 900);   // -1%
    assert_eq!(apply_slippage_min(1000, 2500), 750); // -25%
    assert_eq!(apply_slippage_min(1, 9999), 0);
}

#[test]
fn test_amounts_from_liquidity_burn_q64_boundaries() {
    // 构造一些简单值（Q64.64），假设区间内
    // 注意：函数内部只做数学，不读账户
    let q64 = 1u128 << 64;
    let sa = 2u128 * q64;
    let sb = 4u128 * q64;
    let sp_mid = 3u128 * q64;
    let dliq = 1_000_000u128;

    let (a0, a1) = amounts_from_liquidity_burn_q64(sa, sb, sp_mid, dliq);
    assert!(a0 > 0 || a1 > 0);

    // sp <= sa：只会产生 amount0
    let (a0_only, a1_zero) = amounts_from_liquidity_burn_q64(sa, sb, sa, dliq);
    assert!(a0_only > 0 && a1_zero == 0);

    // sp >= sb：只会产生 amount1
    let (a0_zero, a1_only) = amounts_from_liquidity_burn_q64(sa, sb, sb, dliq);
    assert!(a0_zero == 0 && a1_only > 0);

    // d_liq = 0
    let (z0, z1) = amounts_from_liquidity_burn_q64(sa, sb, sp_mid, 0);
    assert_eq!((z0, z1), (0, 0));
}

#[tokio::test]
#[ignore]
async fn execute_transfer_happy_path_skeleton() {
    // 说明：
    // 1) 由于 Execute 账户结构把 Raydium 池/仓位/tick array/observation 等全体必填并带有强约束，
    //    即使走 Transfer 分支，账户校验也会读取/校验 pool_state 等字段。
    // 2) 因此必须在本地初始化一个 Raydium v3 CLMM 池，创建 protocol/personal position、tick arrays、
    //    vault 和 mint 全套对象，且地址需与 pool_state 内部字段一致。
    // 3) 或者：在 CI 环境引入 Raydium 的测试工具（或“假”CLMM 程序）来产出合法账户快照。
    //
}

#[tokio::test]
#[ignore]
async fn execute_zap_in_out_skeleton() {
    // 与上面的说明一致；不同点是：
    // - 需要能成功调用 Raydium CPI：swap_v2 / open_position_v2 / increase_liquidity_v2 / decrease_liquidity_v2 / close_position
    // - 需要在池子里准备足够流动性与价格，使 tickLower/tickUpper、priceLimit 等参数有效
}