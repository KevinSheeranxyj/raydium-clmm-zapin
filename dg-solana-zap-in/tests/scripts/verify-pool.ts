/**
 * 验证Raydium CLMM池信息的脚本
 * 用于检查raydium.json中的地址是否有效
 */

import * as anchor from "@coral-xyz/anchor";
import { PublicKey, Connection } from "@solana/web3.js";
import * as fs from "fs";
import * as path from "path";

// 加载配置
const raydiumConfig = JSON.parse(
    fs.readFileSync(path.join(__dirname, "../fixtures", "raydium.json"), "utf8")
);

async function verifyPoolInfo() {
    console.log("🔍 验证Raydium CLMM池信息...\n");

    // 连接到devnet
    const connection = new Connection("https://api.devnet.solana.com", "confirmed");

    // 验证程序ID
    console.log("1. 验证CLMM程序ID...");
    const clmmProgramId = new PublicKey(raydiumConfig.CLMM_PROGRAM_ID);
    try {
        const programInfo = await connection.getAccountInfo(clmmProgramId);
        if (programInfo) {
            console.log(`✅ CLMM程序ID有效: ${clmmProgramId.toBase58()}`);
            console.log(`   所有者: ${programInfo.owner.toBase58()}`);
        } else {
            console.log(`❌ CLMM程序ID无效: ${clmmProgramId.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取CLMM程序信息: ${error.message}`);
    }

    // 验证池状态
    console.log("\n2. 验证池状态...");
    const poolState = new PublicKey(raydiumConfig.POOL_STATE);
    try {
        const poolInfo = await connection.getAccountInfo(poolState);
        if (poolInfo) {
            console.log(`✅ 池状态有效: ${poolState.toBase58()}`);
            console.log(`   所有者: ${poolInfo.owner.toBase58()}`);
            console.log(`   数据长度: ${poolInfo.data.length} bytes`);
        } else {
            console.log(`❌ 池状态无效: ${poolState.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取池状态信息: ${error.message}`);
    }

    // 验证AMM配置
    console.log("\n3. 验证AMM配置...");
    const ammConfig = new PublicKey(raydiumConfig.AMM_CONFIG);
    try {
        const configInfo = await connection.getAccountInfo(ammConfig);
        if (configInfo) {
            console.log(`✅ AMM配置有效: ${ammConfig.toBase58()}`);
            console.log(`   所有者: ${configInfo.owner.toBase58()}`);
        } else {
            console.log(`❌ AMM配置无效: ${ammConfig.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取AMM配置信息: ${error.message}`);
    }

    // 验证观察状态
    console.log("\n4. 验证观察状态...");
    try {
        const observationState = new PublicKey(raydiumConfig.OBSERVATION_STATE);
        const obsInfo = await connection.getAccountInfo(observationState);
        if (obsInfo) {
            console.log(`✅ 观察状态有效: ${observationState.toBase58()}`);
            console.log(`   所有者: ${obsInfo.owner.toBase58()}`);
        } else {
            console.log(`❌ 观察状态无效: ${observationState.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取观察状态信息: ${error.message}`);
    }

    // 验证代币金库
    console.log("\n5. 验证代币金库...");
    const tokenVault0 = new PublicKey(raydiumConfig.TOKEN_VAULT_0);
    const tokenVault1 = new PublicKey(raydiumConfig.TOKEN_VAULT_1);
    
    try {
        const vault0Info = await connection.getAccountInfo(tokenVault0);
        if (vault0Info) {
            console.log(`✅ 代币金库0有效: ${tokenVault0.toBase58()}`);
            console.log(`   所有者: ${vault0Info.owner.toBase58()}`);
        } else {
            console.log(`❌ 代币金库0无效: ${tokenVault0.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取代币金库0信息: ${error.message}`);
    }

    try {
        const vault1Info = await connection.getAccountInfo(tokenVault1);
        if (vault1Info) {
            console.log(`✅ 代币金库1有效: ${tokenVault1.toBase58()}`);
            console.log(`   所有者: ${vault1Info.owner.toBase58()}`);
        } else {
            console.log(`❌ 代币金库1无效: ${tokenVault1.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取代币金库1信息: ${error.message}`);
    }

    // 验证代币铸造
    console.log("\n6. 验证代币铸造...");
    const tokenMint0 = new PublicKey(raydiumConfig.TOKEN_MINT_0);
    const tokenMint1 = new PublicKey(raydiumConfig.TOKEN_MINT_1);
    
    try {
        const mint0Info = await connection.getAccountInfo(tokenMint0);
        if (mint0Info) {
            console.log(`✅ 代币铸造0有效: ${tokenMint0.toBase58()}`);
            console.log(`   所有者: ${mint0Info.owner.toBase58()}`);
        } else {
            console.log(`❌ 代币铸造0无效: ${tokenMint0.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取代币铸造0信息: ${error.message}`);
    }

    try {
        const mint1Info = await connection.getAccountInfo(tokenMint1);
        if (mint1Info) {
            console.log(`✅ 代币铸造1有效: ${tokenMint1.toBase58()}`);
            console.log(`   所有者: ${mint1Info.owner.toBase58()}`);
        } else {
            console.log(`❌ 代币铸造1无效: ${tokenMint1.toBase58()}`);
        }
    } catch (error) {
        console.log(`❌ 无法获取代币铸造1信息: ${error.message}`);
    }

    // 显示配置摘要
    console.log("\n📋 配置摘要:");
    console.log(`网络: ${raydiumConfig.network || 'devnet'}`);
    console.log(`池类型: ${raydiumConfig.poolType || 'CLMM'}`);
    console.log(`代币对: ${raydiumConfig.TOKEN_MINT_0} / ${raydiumConfig.TOKEN_MINT_1}`);
    console.log(`Tick间距: ${raydiumConfig.TICK_SPACING}`);
    console.log(`费用率: ${raydiumConfig.feeRate || 'N/A'}`);
    console.log(`协议费用率: ${raydiumConfig.protocolFeeRate || 'N/A'}`);
}

// 运行验证
if (require.main === module) {
    verifyPoolInfo()
        .then(() => {
            console.log("\n✅ 验证完成");
            process.exit(0);
        })
        .catch((error) => {
            console.error("\n❌ 验证失败:", error);
            process.exit(1);
        });
}

export { verifyPoolInfo };
