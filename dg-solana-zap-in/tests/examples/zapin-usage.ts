/**
 * Zap-In操作使用示例
 * 
 * 这个文件展示了如何使用ZapInClient来执行完整的zap-in操作
 */

import * as anchor from "@coral-xyz/anchor";
import { PublicKey, Keypair } from "@solana/web3.js";
import { ZapInClient, ZapInConfig, ZapInParams } from "../helpers/zapin";
import { createUserWithSol, createMintAndATA } from "../helpers/token";
import * as fs from "fs";
import * as path from "path";

// 加载Raydium配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "../fixtures", "raydium.json"), "utf8")
);

/**
 * 基本使用示例
 */
export async function basicZapInExample() {
    // 1. 设置连接和钱包
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    // 2. 加载程序
    const program = anchor.workspace.dgSolanaZapin as Program<any>;

    // 3. 创建测试用户
    const user = await createUserWithSol(provider);
    console.log("User:", user.publicKey.toBase58());

    // 4. 配置ZapIn客户端
    const config: ZapInConfig = {
        program,
        provider,
        poolConfig: {
            clmmProgramId: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
            poolState: new PublicKey(raydiumConfig.POOL_STATE),
            ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
            observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
            tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
            tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
            tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
            tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
            tickSpacing: raydiumConfig.TICK_SPACING,
        }
    };

    const zapInClient = new ZapInClient(config);

    // 5. 创建测试代币
    const { mint: testMint, ata: userAta } = await createMintAndATA(
        provider,
        user.publicKey,
        6, // decimals
        new anchor.BN(1000000) // 1M tokens
    );

    // 6. 定义zap-in参数
    const zapInParams: ZapInParams = {
        amountIn: new anchor.BN(100000), // 100k tokens
        tickLower: raydiumConfig.exampleTicks.tickLower,
        tickUpper: raydiumConfig.exampleTicks.tickUpper,
        slippageBps: 100, // 1% slippage
    };

    // 7. 执行完整的zap-in流程
    try {
        const result = await zapInClient.executeZapIn(zapInParams, user);
        console.log("Zap-in completed successfully!");
        console.log("Transfer ID:", Buffer.from(result.transferId).toString('hex'));
        console.log("Transactions:", result.transactions);
        return result;
    } catch (error) {
        console.error("Zap-in failed:", error);
        throw error;
    }
}

/**
 * 分步执行示例
 */
export async function stepByStepZapInExample() {
    // 设置（同基本示例）
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.dgSolanaZapin as Program<any>;
    const user = await createUserWithSol(provider);

    const config: ZapInConfig = {
        program,
        provider,
        poolConfig: {
            clmmProgramId: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
            poolState: new PublicKey(raydiumConfig.POOL_STATE),
            ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
            observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
            tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
            tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
            tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
            tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
            tickSpacing: raydiumConfig.TICK_SPACING,
        }
    };

    const zapInClient = new ZapInClient(config);
    const transferId = zapInClient.generateTransferId();

    // 创建测试代币
    const { mint: testMint, ata: userAta } = await createMintAndATA(
        provider,
        user.publicKey,
        6,
        new anchor.BN(1000000)
    );

    const zapInParams: ZapInParams = {
        amountIn: new anchor.BN(50000),
        tickLower: raydiumConfig.exampleTicks.tickLower,
        tickUpper: raydiumConfig.exampleTicks.tickUpper,
        slippageBps: 200,
    };

    try {
        // 步骤1: 初始化
        console.log("步骤1: 初始化操作数据...");
        await zapInClient.initialize();
        console.log("✓ 初始化完成");

        // 步骤2: 存入资金
        console.log("步骤2: 存入资金和参数...");
        await zapInClient.deposit(transferId, zapInParams, user, userAta.address);
        console.log("✓ 存入完成");

        // 步骤3: 准备执行
        console.log("步骤3: 准备执行...");
        const refundAta = await getOrCreateAssociatedTokenAccount(
            connection,
            user,
            testMint,
            user.publicKey
        );
        await zapInClient.prepareExecute(transferId, user, refundAta.address);
        console.log("✓ 准备执行完成");

        // 步骤4: 执行swap
        console.log("步骤4: 执行swap操作...");
        await zapInClient.swapForBalance(transferId, user);
        console.log("✓ Swap完成");

        // 步骤5: 打开仓位
        console.log("步骤5: 打开流动性仓位...");
        await zapInClient.openPosition(transferId, user);
        console.log("✓ 打开仓位完成");

        // 步骤6: 增加流动性
        console.log("步骤6: 增加流动性...");
        await zapInClient.increaseLiquidity(transferId, user);
        console.log("✓ 增加流动性完成");

        // 步骤7: 完成执行
        console.log("步骤7: 完成执行...");
        await zapInClient.finalizeExecute(transferId, user);
        console.log("✓ 执行完成");

        console.log("所有步骤执行成功！");
        return { transferId, success: true };

    } catch (error) {
        console.error("执行过程中出错:", error);
        
        // 可以在这里添加取消逻辑
        try {
            console.log("尝试取消操作...");
            await zapInClient.cancel(transferId, user);
            console.log("✓ 取消成功");
        } catch (cancelError) {
            console.error("取消操作失败:", cancelError);
        }
        
        throw error;
    }
}

/**
 * 错误处理示例
 */
export async function errorHandlingExample() {
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.dgSolanaZapin as Program<any>;
    const user = await createUserWithSol(provider);

    const config: ZapInConfig = {
        program,
        provider,
        poolConfig: {
            clmmProgramId: new PublicKey(raydiumConfig.CLMM_PROGRAM_ID),
            poolState: new PublicKey(raydiumConfig.POOL_STATE),
            ammConfig: new PublicKey(raydiumConfig.AMM_CONFIG),
            observationState: new PublicKey(raydiumConfig.OBSERVATION_STATE),
            tokenVault0: new PublicKey(raydiumConfig.TOKEN_VAULT_0),
            tokenVault1: new PublicKey(raydiumConfig.TOKEN_VAULT_1),
            tokenMint0: new PublicKey(raydiumConfig.TOKEN_MINT_0),
            tokenMint1: new PublicKey(raydiumConfig.TOKEN_MINT_1),
            tickSpacing: raydiumConfig.TICK_SPACING,
        }
    };

    const zapInClient = new ZapInClient(config);

    // 测试无效参数
    const invalidParams: ZapInParams = {
        amountIn: new anchor.BN(0), // 无效金额
        tickLower: 100,
        tickUpper: -100, // 无效tick范围
        slippageBps: 15000, // 过高的滑点
    };

    try {
        await zapInClient.executeZapIn(invalidParams, user);
        console.log("不应该到达这里");
    } catch (error) {
        console.log("预期的错误:", error.message);
        // 处理错误...
    }
}

// 如果直接运行此文件
if (require.main === module) {
    console.log("运行Zap-In示例...");
    
    basicZapInExample()
        .then(result => {
            console.log("基本示例完成:", result);
        })
        .catch(error => {
            console.error("基本示例失败:", error);
        });
}
