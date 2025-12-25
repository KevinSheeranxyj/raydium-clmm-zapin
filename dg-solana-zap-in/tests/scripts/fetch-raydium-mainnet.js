/**
 * 获取Raydium CLMM主网池信息的脚本
 */

const fs = require('fs');
const path = require('path');

const RAYDIUM_MAINNET_API = "https://api.raydium.io";

async function fetchRaydiumMainnetPools() {
  console.log("🔍 正在获取Raydium CLMM主网池信息...\n");

  try {
    // 尝试不同的API端点
    const endpoints = [
      `${RAYDIUM_MAINNET_API}/v2/sdk/liquidity/mainnet.json`,
      `${RAYDIUM_MAINNET_API}/v2/sdk/liquidity/mainnet.json`,
      `https://api-v3.raydium.io/pools`,
      `https://api.raydium.io/v2/sdk/liquidity/mainnet.json`
    ];

    let data = null;
    let workingEndpoint = null;

    for (const endpoint of endpoints) {
      try {
        console.log(`尝试端点: ${endpoint}`);
        const response = await fetch(endpoint);
        
        if (response.ok) {
          data = await response.json();
          workingEndpoint = endpoint;
          console.log(`✅ 成功从 ${endpoint} 获取数据`);
          break;
        } else {
          console.log(`❌ ${endpoint} 返回状态: ${response.status}`);
        }
      } catch (error) {
        console.log(`❌ ${endpoint} 请求失败: ${error.message}`);
      }
    }

    if (!data) {
      throw new Error("所有API端点都失败了");
    }

    console.log(`✅ 成功获取数据，类型: ${typeof data}`);

    // 如果是数组，直接处理
    let pools = Array.isArray(data) ? data : [];

    // 如果是对象，尝试找到池数组
    if (!Array.isArray(data) && data.pools) {
      pools = data.pools;
    }

    console.log(`📊 找到 ${pools.length} 个池`);

    // 过滤CLMM池
    const clmmPools = pools.filter(pool => 
      pool.programId === "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK" ||
      pool.programId === "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"
    );

    console.log(`🎯 找到 ${clmmPools.length} 个CLMM池`);

    // 查找SOL/USDC池
    const solUsdcPools = clmmPools.filter(pool => 
      (pool.baseMint === "So11111111111111111111111111111111111111112" && 
       pool.quoteMint === "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v") ||
      (pool.baseMint === "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" && 
       pool.quoteMint === "So11111111111111111111111111111111111111112")
    );

    console.log(`💰 找到 ${solUsdcPools.length} 个SOL/USDC池`);

    if (solUsdcPools.length > 0) {
      const pool = solUsdcPools[0];
      console.log("\n🎯 推荐的SOL/USDC池:");
      console.log(`池ID: ${pool.id || pool.poolId || 'N/A'}`);
      console.log(`池状态: ${pool.poolState || pool.id || 'N/A'}`);
      console.log(`AMM配置: ${pool.ammConfig || 'N/A'}`);
      console.log(`观察ID: ${pool.observationId || pool.observationState || 'N/A'}`);
      console.log(`基础代币: ${pool.baseMint}`);
      console.log(`报价代币: ${pool.quoteMint}`);
      console.log(`基础金库: ${pool.baseVault || 'N/A'}`);
      console.log(`报价金库: ${pool.quoteVault || 'N/A'}`);
      console.log(`Tick间距: ${pool.tickSpacing || 1}`);

      // 生成配置文件
      const config = {
        CLMM_PROGRAM_ID: "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
        POOL_STATE: pool.poolState || pool.id || "8BnEgHoWFysVcuFFX7QztDmzuH8r5ZFvyP3sYwn1XTh6",
        AMM_CONFIG: pool.ammConfig || "2QdhepnKRTLjjSqj1oeoRjy7PJZ7RX9Q9FdcQzq6BEin",
        OBSERVATION_STATE: pool.observationId || pool.observationState || "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQffMztkOvEDB",
        TOKEN_VAULT_0: pool.baseVault || "FgZut2qVQEyPBibaTJbbX2PxaM6vT1Sqr1D6A2inD9sP",
        TOKEN_VAULT_1: pool.quoteVault || "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
        TOKEN_MINT_0: pool.baseMint,
        TOKEN_MINT_1: pool.quoteMint,
        TICK_SPACING: pool.tickSpacing || 1,
        SQRT_PRICE_X64: "79228162514264337593543950336",
        exampleTicks: {
          tickLower: -120,
          tickUpper: 120
        },
        description: "Raydium CLMM SOL/USDC pool from mainnet API",
        network: "mainnet",
        poolType: "CLMM",
        feeRate: 0.0001,
        protocolFeeRate: 0.0001,
        source: "Raydium Mainnet API",
        lastUpdated: new Date().toISOString(),
        workingEndpoint: workingEndpoint
      };

      // 保存配置
      const configPath = path.join(__dirname, "../fixtures/raydium-mainnet.json");
      fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
      console.log(`\n💾 配置已保存到: ${configPath}`);

      return config;
    } else {
      console.log("❌ 未找到SOL/USDC池");
      console.log("可用的池类型:", [...new Set(pools.map(p => p.programId))]);
      return null;
    }

  } catch (error) {
    console.error("❌ 获取池信息失败:", error);
    return null;
  }
}

// 运行脚本
if (require.main === module) {
  fetchRaydiumMainnetPools()
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

module.exports = { fetchRaydiumMainnetPools };
