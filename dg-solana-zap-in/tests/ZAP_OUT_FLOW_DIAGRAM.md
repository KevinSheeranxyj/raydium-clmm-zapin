# Zap-Out 流程图

## 整体流程

```
开始
  ↓
验证transfer_id和操作状态
  ↓
提取Raydium相关地址和账户
  ↓
计算预期输出金额（含滑点保护）
  ↓
执行DecreaseLiquidityV2（提取流动性）
  ↓
计算实际获得的代币数量
  ↓
判断是否需要代币交换
  ↓
[需要交换] → 执行SwapSingleV2 → 更新最终金额
  ↓
[不需要交换] → 直接使用现有金额
  ↓
检查最小到手金额保护
  ↓
转账给用户
  ↓
标记执行完成
  ↓
结束
```

## 详细步骤

### 1. 验证阶段
```
输入: transfer_id, ZapOutParams
  ↓
检查transfer_id是否已使用（防重复）
  ↓
验证operation_data状态（已初始化、未执行、金额>0）
  ↓
确定接收者（recipient或authority）
  ↓
提取Raydium相关地址
```

### 2. 流动性计算阶段
```
读取PersonalPositionState
  ↓
确定要燃烧的流动性数量
  ↓
计算tick范围和当前价格
  ↓
使用tick数学计算预期输出
  ↓
应用滑点保护计算最小输出
```

### 3. 流动性提取阶段
```
记录提取前PDA代币账户余额
  ↓
调用DecreaseLiquidityV2
  ↓
记录提取后PDA代币账户余额
  ↓
计算实际获得的代币数量
```

### 4. 代币交换阶段
```
根据want_base参数确定交换方向
  ↓
[want_base=true] → 交换token1→token0
  ↓
[want_base=false] → 交换token0→token1
  ↓
调用SwapSingleV2执行交换
  ↓
更新最终输出金额
```

### 5. 转账完成阶段
```
检查最终金额是否满足min_payout
  ↓
从PDA代币账户转账到用户账户
  ↓
标记operation_data.executed = true
  ↓
完成
```

## 关键决策点

### 决策点1: 流动性提取数量
```
if liquidity_to_burn_u64 > 0:
    burn_liq = liquidity_to_burn_u64
else:
    burn_liq = full_liquidity  // 提取全部
```

### 决策点2: 代币交换方向
```
if want_base:
    // 想要token0，交换token1→token0
    swap_amount = got1
    is_base_input = false
else:
    // 想要token1，交换token0→token1
    swap_amount = got0
    is_base_input = true
```

### 决策点3: 是否需要交换
```
if swap_amount > 0:
    execute_swap()
else:
    skip_swap()
```

### 决策点4: 最小金额检查
```
if total_out >= min_payout:
    proceed_with_transfer()
else:
    throw_error()
```

## 账户关系图

```
用户 (user)
  ↓
操作数据PDA (operation_data)
  ↓
PDA代币账户 (input_token_account, output_token_account)
  ↓
Raydium池 (pool_state, vaults, mints)
  ↓
Position NFT (position_nft_mint, position_nft_account)
  ↓
用户接收账户 (recipient_token_account)
```

## 安全机制

### 1. 防重复执行
```
if registry.used_ids.contains(transfer_id_hash):
    throw DuplicateTransferId
if operation_data.executed:
    throw AlreadyExecuted
```

### 2. 权限控制
```
if recipient_token_account.owner != expected_recipient:
    throw Unauthorized
```

### 3. 金额保护
```
if total_out < min_payout:
    throw InvalidParams
```

### 4. 滑点保护
```
min_amount = estimated_amount * (1 - slippage_bps / 10000)
```

## 错误处理路径

```
DuplicateTransferId → 停止执行
NotInitialized → 停止执行
AlreadyExecuted → 停止执行
InvalidAmount → 停止执行
Unauthorized → 停止执行
InvalidMint → 停止执行
InvalidParams → 停止执行
InvalidTickRange → 停止执行
Insufficient payout → 停止执行
Transfer failed → 停止执行
```

## 与标准LP Zap-Out对比

| 功能 | 标准实现 | 本实现 | 状态 |
|------|----------|--------|------|
| 流动性提取 | DecreaseLiquidity | DecreaseLiquidityV2 | ✅ |
| 代币分离 | 自动分离 | 自动分离 | ✅ |
| 可选交换 | 单边交换 | 单边交换 | ✅ |
| 滑点保护 | 最小输出 | 最小输出 | ✅ |
| 权限控制 | 用户授权 | 用户授权 | ✅ |
| 金额保护 | 最小金额 | 最小金额 | ✅ |
| 状态管理 | 防重复 | 防重复 | ✅ |

## 优化建议

### 1. 代码结构
- 将helper函数移到helpers.rs
- 添加更详细的事件记录
- 优化错误信息

### 2. 功能增强
- 支持批量操作
- 添加更多代币标准支持
- 实现更复杂的交换策略

### 3. 性能优化
- 减少不必要的账户查找
- 优化计算逻辑
- 添加缓存机制

这个zap-out实现完全符合标准LP退出逻辑，提供了完整、安全、用户友好的流动性提取功能。
