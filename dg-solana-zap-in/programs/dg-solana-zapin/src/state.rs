use anchor_lang::prelude::*;

/// 主执行状态，按子指令推进
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExecStage {
    None,          // deposit 后尚未开始
    Prepared,      // 完成 prepare_execute
    Swapped,       // 完成 swap_for_balance
    Opened,        // 完成 open_position_step
    LiquidityAdded,// 完成 increase_liquidity_step
    Finalized,     // 全流程结束（成功或 cancel）
}
impl Default for ExecStage {
    fn default() -> Self { ExecStage::None }
}

/// 操作类型
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum OperationType {
    Transfer,
    ZapIn,
}
impl Default for OperationType {
    fn default() -> Self { OperationType::Transfer }
}

/// ZapIn 参数
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ZapInParams {
    pub amount_in: u64,   // required
    pub pool: Pubkey,     // required
    pub tick_lower: i32,  // required
    pub tick_upper: i32,  // required
    pub slippage_bps: u32,// required
}

/// Transfer 参数
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TransferParams {
    pub amount: u64,
    pub recipient: Pubkey,
}

/// 记录去重用的 Registry 账户
#[account]
pub struct Registry {
    pub used_ids: Vec<[u8; 32]>,
}
impl Registry {
    pub const START_CAP: usize = 32; // 初始容量
    pub const LEN: usize = 4 + Self::START_CAP * 32;
}