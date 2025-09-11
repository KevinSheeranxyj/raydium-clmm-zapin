
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
