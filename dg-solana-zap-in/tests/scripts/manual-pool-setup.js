/**
 * 手动设置Raydium CLMM池信息的脚本
 * 提供多种获取池信息的方法
 */

const fs = require('fs');
const path = require('path');

// 已知的Raydium CLMM程序ID
const RAYDIUM_CLMM_PROGRAM_ID = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

// 常见的代币地址
const TOKENS = {
  SOL: "So11111111111111111111111111111111111111112",
  USDC: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  USDT: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
  RAY: "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R"
};

function createExampleConfig() {
  console.log("🔧 创建示例Raydium CLMM配置...\n");

  // 示例配置 - 这些是示例地址，需要替换为真实的池地址
  const exampleConfig = {
    CLMM_PROGRAM_ID: RAYDIUM_CLMM_PROGRAM_ID,
    POOL_STATE: "8BnEgHoWFysVcuFFX7QztDmzuH8r5ZFvyP3sYwn1XTh6",
    AMM_CONFIG: "2QdhepnKRTLjjSqj1oeoRjy7PJZ7RX9Q9FdcQzq6BEin",
    OBSERVATION_STATE: "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQffMztkOvEDB",
    TOKEN_VAULT_0: "FgZut2qVQEyPBibaTJbbX2PxaM6vT1Sqr1D6A2inD9sP",
    TOKEN_VAULT_1: "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R",
    TOKEN_MINT_0: TOKENS.SOL,
    TOKEN_MINT_1: TOKENS.USDC,
    TICK_SPACING: 1,
    SQRT_PRICE_X64: "79228162514264337593543950336",
    exampleTicks: {
      tickLower: -120,
      tickUpper: 120
    },
    description: "Raydium CLMM SOL/USDC pool configuration (example addresses)",
    network: "devnet",
    poolType: "CLMM",
    feeRate: 0.0001,
    protocolFeeRate: 0.0001,
    note: "These are example addresses. Replace with actual pool addresses.",
    instructions: {
      howToGetRealAddresses: [
        "1. Visit Raydium's interface: https://raydium.io/",
        "2. Go to Pools section and find CLMM pools",
        "3. Select a SOL/USDC pool",
        "4. Copy the pool address and related addresses",
        "5. Use Solana Explorer to verify addresses",
        "6. Replace the example addresses in this config"
      ],
      alternativeMethods: [
        "Use Raydium SDK to query pool information",
        "Use Solana RPC to get account information",
        "Check Raydium's GitHub for pool addresses",
        "Use third-party APIs like Jupiter or Birdeye"
      ]
    }
  };

  // 保存示例配置
  const configPath = path.join(__dirname, "../fixtures/raydium-example.json");
  fs.writeFileSync(configPath, JSON.stringify(exampleConfig, null, 2));
  console.log(`✅ 示例配置已保存到: ${configPath}`);

  return exampleConfig;
}

function printInstructions() {
  console.log("\n📋 获取Raydium CLMM池信息的方法:\n");

  console.log("1. 🌐 通过Raydium网站:");
  console.log("   - 访问 https://raydium.io/");
  console.log("   - 进入 Pools 页面");
  console.log("   - 找到 CLMM 池");
  console.log("   - 点击池详情查看地址");

  console.log("\n2. 🔍 通过Solana Explorer:");
  console.log("   - 访问 https://explorer.solana.com/");
  console.log("   - 搜索 Raydium CLMM 程序ID: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");
  console.log("   - 查看程序账户找到池地址");

  console.log("\n3. 📚 通过Raydium SDK:");
  console.log("   - 安装: npm install @raydium-io/raydium-sdk");
  console.log("   - 使用 SDK 查询池信息");

  console.log("\n4. 🔧 通过RPC调用:");
  console.log("   - 使用 getProgramAccounts 查询程序账户");
  console.log("   - 解析账户数据获取池信息");

  console.log("\n5. 📊 通过第三方API:");
  console.log("   - Jupiter API: https://quote-api.jup.ag/");
  console.log("   - Birdeye API: https://public-api.birdeye.so/");
  console.log("   - DexScreener API: https://api.dexscreener.com/");

  console.log("\n6. 🛠️ 手动创建测试池:");
  console.log("   - 使用 Raydium SDK 创建测试池");
  console.log("   - 在 devnet 上部署测试池");
  console.log("   - 获取池地址用于测试");
}

function createRPCQueryScript() {
  const rpcScript = `
// 使用Solana RPC查询Raydium CLMM池的示例脚本
const { Connection, PublicKey } = require('@solana/web3.js');

const RAYDIUM_CLMM_PROGRAM_ID = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

async function queryRaydiumPools() {
  const connection = new Connection('https://api.devnet.solana.com');
  
  try {
    // 获取程序的所有账户
    const accounts = await connection.getProgramAccounts(
      new PublicKey(RAYDIUM_CLMM_PROGRAM_ID)
    );
    
    console.log(\`找到 \${accounts.length} 个账户\`);
    
    // 解析池账户
    for (const account of accounts) {
      console.log(\`账户: \${account.pubkey.toBase58()}\`);
      console.log(\`数据长度: \${account.account.data.length}\`);
      console.log(\`所有者: \${account.account.owner.toBase58()}\`);
      console.log('---');
    }
  } catch (error) {
    console.error('查询失败:', error);
  }
}

queryRaydiumPools();
`;

  const scriptPath = path.join(__dirname, "query-raydium-pools.js");
  fs.writeFileSync(scriptPath, rpcScript);
  console.log(`\n📝 RPC查询脚本已保存到: ${scriptPath}`);
}

function createSDKExample() {
  const sdkExample = `
// 使用Raydium SDK查询池信息的示例
const { Raydium } = require('@raydium-io/raydium-sdk');

async function queryPoolsWithSDK() {
  try {
    // 初始化SDK
    const raydium = new Raydium();
    
    // 获取所有池
    const pools = await raydium.getPools();
    
    // 过滤CLMM池
    const clmmPools = pools.filter(pool => 
      pool.programId === 'CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK'
    );
    
    // 查找SOL/USDC池
    const solUsdcPools = clmmPools.filter(pool => 
      (pool.baseMint === 'So11111111111111111111111111111111111111112' && 
       pool.quoteMint === 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v')
    );
    
    console.log('找到的SOL/USDC池:', solUsdcPools);
  } catch (error) {
    console.error('SDK查询失败:', error);
  }
}

queryPoolsWithSDK();
`;

  const sdkPath = path.join(__dirname, "query-with-sdk.js");
  fs.writeFileSync(sdkPath, sdkExample);
  console.log(`📝 SDK示例已保存到: ${sdkPath}`);
}

// 主函数
function main() {
  console.log("🚀 Raydium CLMM池信息获取工具\n");
  
  // 创建示例配置
  createExampleConfig();
  
  // 打印说明
  printInstructions();
  
  // 创建RPC查询脚本
  createRPCQueryScript();
  
  // 创建SDK示例
  createSDKExample();
  
  console.log("\n✅ 所有工具和示例已创建完成！");
  console.log("\n下一步:");
  console.log("1. 选择一个方法获取真实的池地址");
  console.log("2. 替换 raydium-example.json 中的示例地址");
  console.log("3. 运行验证脚本测试配置");
}

// 运行主函数
if (require.main === module) {
  main();
}

module.exports = { createExampleConfig, printInstructions };
