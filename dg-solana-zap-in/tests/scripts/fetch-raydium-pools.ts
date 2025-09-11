/**
 * 获取Raydium CLMM devnet池信息的脚本
 */

import * as fs from "fs";
import * as path from "path";

const RAYDIUM_DEVNET_API = "https://api-v3-devnet.raydium.io";

interface RaydiumPool {
  id: string;
  baseMint: string;
  quoteMint: string;
  baseVault: string;
  quoteVault: string;
  baseDecimals: number;
  quoteDecimals: number;
  tickSpacing: number;
  ammConfig: string;
  observationId: string;
  poolState: string;
  programId: string;
  status: string;
}

async function fetchRaydiumPools() {
  console.log("🔍 正在获取Raydium CLMM devnet池信息...\n");

  try {
    // 获取所有池信息
    const response = await fetch(`${RAYDIUM_DEVNET_API}/pools`);
    
    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }

    const data = await response.json();
    console.log(`✅ 成功获取到 ${data.length} 个池`);

    // 过滤CLMM池
    const clmmPools = data.filter((pool: any) => 
      pool.programId === "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"
    );

    console.log(`📊 找到 ${clmmPools.length} 个CLMM池`);

    // 查找SOL/USDC池
    const solUsdcPools = clmmPools.filter((pool: any) => 
      (pool.baseMint === "So11111111111111111111111111111111111111112" && 
       pool.quoteMint === "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v") ||
      (pool.baseMint === "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" && 
       pool.quoteMint === "So11111111111111111111111111111111111111112")
    );

    console.log(`💰 找到 ${solUsdcPools.length} 个SOL/USDC池`);

    if (solUsdcPools.length > 0) {
      const pool = solUsdcPools[0];
      console.log("\n🎯 推荐的SOL/USDC池:");
      console.log(`池ID: ${pool.id}`);
      console.log(`池状态: ${pool.poolState}`);
      console.log(`AMM配置: ${pool.ammConfig}`);
      console.log(`观察ID: ${pool.observationId}`);
      console.log(`基础代币: ${pool.baseMint}`);
      console.log(`报价代币: ${pool.quoteMint}`);
      console.log(`基础金库: ${pool.baseVault}`);
      console.log(`报价金库: ${pool.quoteVault}`);
      console.log(`Tick间距: ${pool.tickSpacing}`);

      // 生成配置文件
      const config = {
        CLMM_PROGRAM_ID: pool.programId,
        POOL_STATE: pool.poolState,
        AMM_CONFIG: pool.ammConfig,
        OBSERVATION_STATE: pool.observationId,
        TOKEN_VAULT_0: pool.baseVault,
        TOKEN_VAULT_1: pool.quoteVault,
        TOKEN_MINT_0: pool.baseMint,
        TOKEN_MINT_1: pool.quoteMint,
        TICK_SPACING: pool.tickSpacing,
        SQRT_PRICE_X64: "0",
        exampleTicks: {
          tickLower: -120,
          tickUpper: 120
        },
        description: "Raydium CLMM SOL/USDC pool from devnet API",
        network: "devnet",
        poolType: "CLMM",
        feeRate: 0.0001,
        protocolFeeRate: 0.0001,
        source: "Raydium Devnet API",
        lastUpdated: new Date().toISOString()
      };

      // 保存配置
      const configPath = path.join(__dirname, "../fixtures/raydium-devnet.json");
      fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
      console.log(`\n💾 配置已保存到: ${configPath}`);

      return config;
    } else {
      console.log("❌ 未找到SOL/USDC池");
      return null;
    }

  } catch (error) {
    console.error("❌ 获取池信息失败:", error);
    return null;
  }
}

// 运行脚本
if (require.main === module) {
  fetchRaydiumPools()
    .then((config) => {
      if (config) {
        console.log("\n✅ 成功获取并保存池配置");
      } else {
        console.log("\n❌ 获取池配置失败");
      }
    })
    .catch((error) => {
      console.error("❌ 脚本执行失败:", error);
    });
}

export { fetchRaydiumPools };
