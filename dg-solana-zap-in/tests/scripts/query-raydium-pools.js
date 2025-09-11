
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
    
    console.log(`找到 ${accounts.length} 个账户`);
    
    // 解析池账户
    for (const account of accounts) {
      console.log(`账户: ${account.pubkey.toBase58()}`);
      console.log(`数据长度: ${account.account.data.length}`);
      console.log(`所有者: ${account.account.owner.toBase58()}`);
      console.log('---');
    }
  } catch (error) {
    console.error('查询失败:', error);
  }
}

queryRaydiumPools();
