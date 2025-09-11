# Zap-In TypeScript 调用指南

这个目录包含了用于与 dg-solana-zapin 程序交互的 TypeScript 客户端和测试文件。

## 文件结构

```
tests/
├── helpers/
│   ├── zapin.ts          # ZapInClient 主类
│   ├── params.ts         # 参数编码工具
│   ├── pdas.ts           # PDA 计算工具
│   └── token.ts          # 代币操作工具
├── examples/
│   └── zapin-usage.ts    # 使用示例
├── fixtures/
│   └── raydium.json      # Raydium 配置
├── zapin.simple.spec.ts  # 简单测试
├── zapin.complete.spec.ts # 完整测试
└── README.md             # 本文档
```

## 快速开始

### 1. 基本设置

```typescript
import * as anchor from "@coral-xyz/anchor";
import { ZapInClient, ZapInConfig } from "./helpers/zapin";

// 设置连接
const connection = new anchor.web3.Connection("YOUR_RPC_URL");
const wallet = anchor.Wallet.local();
const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
anchor.setProvider(provider);

// 加载程序
const program = anchor.workspace.dgSolanaZapin as Program<any>;

// 配置 ZapIn 客户端
const config: ZapInConfig = {
    program,
    provider,
    poolConfig: {
        clmmProgramId: new PublicKey("RAYDIUM_CLMM_PROGRAM_ID"),
        poolState: new PublicKey("POOL_STATE"),
        ammConfig: new PublicKey("AMM_CONFIG"),
        observationState: new PublicKey("OBSERVATION_STATE"),
        tokenVault0: new PublicKey("TOKEN_VAULT_0"),
        tokenVault1: new PublicKey("TOKEN_VAULT_1"),
        tokenMint0: new PublicKey("TOKEN_MINT_0"),
        tokenMint1: new PublicKey("TOKEN_MINT_1"),
        tickSpacing: 1,
    }
};

const zapInClient = new ZapInClient(config);
```

### 2. 执行完整的 Zap-In 操作

```typescript
import { ZapInParams } from "./helpers/zapin";

// 定义参数
const zapInParams: ZapInParams = {
    amountIn: new anchor.BN(100000), // 输入金额
    tickLower: -120,                 // 下tick
    tickUpper: 120,                  // 上tick
    slippageBps: 100,               // 滑点 (1%)
};

// 创建用户
const user = Keypair.generate();

// 执行完整的 zap-in 流程
const result = await zapInClient.executeZapIn(zapInParams, user);
console.log("Transfer ID:", Buffer.from(result.transferId).toString('hex'));
console.log("Transactions:", result.transactions);
```

### 3. 分步执行

```typescript
// 生成 transfer ID
const transferId = zapInClient.generateTransferId();

// 步骤1: 初始化
await zapInClient.initialize();

// 步骤2: 存入资金
await zapInClient.deposit(transferId, zapInParams, user, userAta);

// 步骤3: 准备执行
await zapInClient.prepareExecute(transferId, user, refundAta);

// 步骤4: 执行 swap
await zapInClient.swapForBalance(transferId, user);

// 步骤5: 打开仓位
await zapInClient.openPosition(transferId, user);

// 步骤6: 增加流动性
await zapInClient.increaseLiquidity(transferId, user);

// 步骤7: 完成执行
await zapInClient.finalizeExecute(transferId, user);
```

## Zap-In 操作流程

Zap-In 操作包含以下步骤：

1. **initialize()** - 初始化操作数据 PDA
2. **deposit()** - 存入资金和操作参数
3. **prepareExecute()** - 准备执行，转移资金到 PDA 账户
4. **swapForBalance()** - 执行 swap 操作平衡代币
5. **openPosition()** - 打开流动性仓位
6. **increaseLiquidity()** - 增加流动性
7. **finalizeExecute()** - 完成执行

## 错误处理

```typescript
try {
    const result = await zapInClient.executeZapIn(zapInParams, user);
    console.log("成功:", result);
} catch (error) {
    console.error("操作失败:", error);
    
    // 可以尝试取消操作
    try {
        await zapInClient.cancel(transferId, user);
        console.log("取消成功");
    } catch (cancelError) {
        console.error("取消失败:", cancelError);
    }
}
```

## 参数说明

### ZapInParams

```typescript
interface ZapInParams {
    amountIn: anchor.BN;    // 输入代币数量
    tickLower: number;      // 下tick值
    tickUpper: number;      // 上tick值
    slippageBps: number;    // 滑点容忍度 (基点，100 = 1%)
}
```

### ZapInConfig

```typescript
interface ZapInConfig {
    program: anchor.Program;           // Anchor 程序实例
    provider: anchor.AnchorProvider;   // Anchor 提供者
    poolConfig: {
        clmmProgramId: PublicKey;      // Raydium CLMM 程序 ID
        poolState: PublicKey;          // 池状态
        ammConfig: PublicKey;          // AMM 配置
        observationState: PublicKey;   // 观察状态
        tokenVault0: PublicKey;        // 代币金库 0
        tokenVault1: PublicKey;        // 代币金库 1
        tokenMint0: PublicKey;         // 代币铸造 0
        tokenMint1: PublicKey;         // 代币铸造 1
        tickSpacing: number;           // Tick 间距
    };
}
```

## 运行测试

```bash
# 运行简单测试
npm test -- zapin.simple.spec.ts

# 运行完整测试
npm test -- zapin.complete.spec.ts

# 运行所有测试
npm test
```

## 注意事项

1. **网络配置**: 确保使用正确的 RPC URL 和网络
2. **代币账户**: 确保用户有足够的代币余额
3. **权限**: 确保用户有执行操作的权限
4. **滑点**: 合理设置滑点容忍度以避免交易失败
5. **Tick 范围**: 确保 tick 范围在池的有效范围内

## 故障排除

### 常见错误

1. **Insufficient funds**: 用户代币余额不足
2. **Invalid tick range**: tick 范围无效
3. **Slippage exceeded**: 滑点超过容忍度
4. **Account not found**: 账户不存在
5. **Unauthorized**: 权限不足

### 调试技巧

1. 使用 `console.log` 记录关键信息
2. 检查账户状态和余额
3. 验证 PDA 计算是否正确
4. 确认程序 ID 和网络配置

## 示例代码

查看 `examples/zapin-usage.ts` 文件获取更多使用示例。
