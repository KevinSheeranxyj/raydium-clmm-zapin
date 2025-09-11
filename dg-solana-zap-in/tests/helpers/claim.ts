import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { Program, AnchorProvider } from "@coral-xyz/anchor";
import { TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID } from "@solana/spl-token";

export interface ClaimConfig {
    program: Program<any>;
    provider: AnchorProvider;
    poolConfig: {
        clmmProgramId: PublicKey;
        poolState: PublicKey;
        ammConfig: PublicKey;
        observationState: PublicKey;
        tokenVault0: PublicKey;
        tokenVault1: PublicKey;
        tokenMint0: PublicKey;
        tokenMint1: PublicKey;
        tickSpacing: number;
    };
}

export interface ClaimParams {
    minPayout: number; // 最小到手金额
}

export class ClaimClient {
    private program: Program<any>;
    private provider: AnchorProvider;
    private poolConfig: ClaimConfig['poolConfig'];

    constructor(config: ClaimConfig) {
        this.program = config.program;
        this.provider = config.provider;
        this.poolConfig = config.poolConfig;
    }

    /**
     * 执行claim操作（新版本，不需要transfer_id）
     */
    async claim(
        params: ClaimParams,
        user: Keypair,
        nftMint: PublicKey,
        remainingAccounts: PublicKey[]
    ): Promise<string> {
        // 计算claim PDA
        const [claimPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("claim_pda"), user.publicKey.toBuffer(), nftMint.toBuffer()],
            this.program.programId
        );

        // 计算NFT账户
        const nftAccount = PublicKey.findAssociatedTokenAddressSync(
            user.publicKey,
            nftMint
        );

        // 计算接收账户（假设用户想要接收token0）
        const recipientTokenAccount = PublicKey.findAssociatedTokenAddressSync(
            user.publicKey,
            this.poolConfig.tokenMint0
        );

        const tx = await this.program.methods
            .claim({ minPayout: new this.program.BN(params.minPayout) })
            .accounts({
                user: user.publicKey,
                memoProgram: new PublicKey("MemoSq4gqABAX3b7cqRUpy4U5M1ZcvTd5Z73s3J6"),
                clmmProgram: this.poolConfig.clmmProgramId,
                tokenProgram: TOKEN_PROGRAM_ID,
                tokenProgram2022: new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
                systemProgram: SystemProgram.programId,
                poolState: this.poolConfig.poolState,
                ammConfig: this.poolConfig.ammConfig,
                observationState: this.poolConfig.observationState,
                protocolPosition: remainingAccounts[0], // 从remaining accounts获取
                personalPosition: remainingAccounts[1],
                tickArrayLower: remainingAccounts[2],
                tickArrayUpper: remainingAccounts[3],
                tokenVault0: this.poolConfig.tokenVault0,
                tokenVault1: this.poolConfig.tokenVault1,
                tokenMint0: this.poolConfig.tokenMint0,
                tokenMint1: this.poolConfig.tokenMint1,
                nftAccount: nftAccount,
                recipientTokenAccount: recipientTokenAccount,
            })
            .signers([user])
            .rpc();

        return tx;
    }

    /**
     * 构建claim所需的remaining accounts
     */
    buildRemainingAccounts(
        user: Keypair,
        nftMint: PublicKey,
        tickLower: number,
        tickUpper: number
    ): PublicKey[] {
        // 计算tick array PDAs
        const [tickArrayLower, tickArrayUpper] = this.calculateTickArrayPdas(tickLower, tickUpper);
        
        // 计算position PDAs
        const [protocolPosition, personalPosition] = this.calculatePositionPdas(tickLower, tickUpper);

        return [
            // Programs
            TOKEN_PROGRAM_ID,
            this.poolConfig.clmmProgramId,
            new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"), // Token2022
            new PublicKey("MemoSq4gqABAX3b7cqRUpy4U5M1ZcvTd5Z73s3J6"), // Memo
            user.publicKey,

            // Raydium accounts
            this.poolConfig.poolState,
            this.poolConfig.ammConfig,
            this.poolConfig.observationState,
            this.poolConfig.tokenVault0,
            this.poolConfig.tokenVault1,
            this.poolConfig.tokenMint0,
            this.poolConfig.tokenMint1,

            // Tick arrays
            tickArrayLower,
            tickArrayUpper,

            // Positions
            protocolPosition,
            personalPosition,

            // Position NFT
            PublicKey.findAssociatedTokenAddressSync(user.publicKey, nftMint),

            // Recipient token account (假设用户想要接收token0)
            PublicKey.findAssociatedTokenAddressSync(user.publicKey, this.poolConfig.tokenMint0),
        ];
    }

    /**
     * 计算tick array PDAs
     */
    calculateTickArrayPdas(tickLower: number, tickUpper: number): [PublicKey, PublicKey] {
        const tickSpacing = this.poolConfig.tickSpacing;
        
        // 计算tick array起始索引
        const lowerStart = Math.floor(tickLower / tickSpacing) * tickSpacing;
        const upperStart = Math.floor(tickUpper / tickSpacing) * tickSpacing;

        const [tickArrayLower] = PublicKey.findProgramAddressSync(
            [
                Buffer.from("tick_array"),
                this.poolConfig.poolState.toBuffer(),
                Buffer.from(lowerStart.toString().padStart(8, '0')),
            ],
            this.poolConfig.clmmProgramId
        );

        const [tickArrayUpper] = PublicKey.findProgramAddressSync(
            [
                Buffer.from("tick_array"),
                this.poolConfig.poolState.toBuffer(),
                Buffer.from(upperStart.toString().padStart(8, '0')),
            ],
            this.poolConfig.clmmProgramId
        );

        return [tickArrayLower, tickArrayUpper];
    }

    /**
     * 计算position PDAs
     */
    calculatePositionPdas(tickLower: number, tickUpper: number): [PublicKey, PublicKey] {
        const tickSpacing = this.poolConfig.tickSpacing;
        
        // 计算tick array起始索引
        const lowerStart = Math.floor(tickLower / tickSpacing) * tickSpacing;
        const upperStart = Math.floor(tickUpper / tickSpacing) * tickSpacing;

        const [protocolPosition] = PublicKey.findProgramAddressSync(
            [
                Buffer.from("position"),
                this.poolConfig.poolState.toBuffer(),
                Buffer.from(lowerStart.toString().padStart(8, '0')),
                Buffer.from(upperStart.toString().padStart(8, '0')),
            ],
            this.poolConfig.clmmProgramId
        );

        const [personalPosition] = PublicKey.findProgramAddressSync(
            [
                Buffer.from("personal_position"),
                this.poolConfig.poolState.toBuffer(),
                Buffer.from(lowerStart.toString().padStart(8, '0')),
                Buffer.from(upperStart.toString().padStart(8, '0')),
            ],
            this.poolConfig.clmmProgramId
        );

        return [protocolPosition, personalPosition];
    }

    /**
     * 完整的claim流程（新版本）
     */
    async executeClaim(
        params: ClaimParams,
        user: Keypair,
        nftMint: PublicKey,
        tickLower: number,
        tickUpper: number
    ): Promise<string> {
        console.log("Starting claim process...");

        // 计算所有必要的PDAs
        const [claimPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("claim_pda"), user.publicKey.toBuffer(), nftMint.toBuffer()],
            this.program.programId
        );

        const nftAccount = PublicKey.findAssociatedTokenAddressSync(
            user.publicKey,
            nftMint
        );

        const [tickArrayLower, tickArrayUpper] = this.calculateTickArrayPdas(tickLower, tickUpper);
        const [protocolPosition, personalPosition] = this.calculatePositionPdas(tickLower, tickUpper);

        // 构建remaining accounts
        const remainingAccounts = this.buildRemainingAccounts(user, nftMint, tickLower, tickUpper);

        console.log("Claim accounts prepared, executing...");

        // 执行claim
        const tx = await this.claim(params, user, nftMint, remainingAccounts);

        console.log("Claim completed:", tx);
        return tx;
    }

    /**
     * 验证claim前置条件
     */
    async validateClaimPreconditions(
        user: Keypair,
        nftMint: PublicKey
    ): Promise<boolean> {
        try {
            // 检查NFT账户是否存在
            const nftAccount = PublicKey.findAssociatedTokenAddressSync(
                user.publicKey,
                nftMint
            );

            const nftAccountInfo = await this.provider.connection.getAccountInfo(nftAccount);
            if (!nftAccountInfo) {
                console.log("NFT account does not exist");
                return false;
            }

            // 检查接收账户是否存在
            const recipientAccount = PublicKey.findAssociatedTokenAddressSync(
                user.publicKey,
                this.poolConfig.tokenMint0
            );

            const recipientAccountInfo = await this.provider.connection.getAccountInfo(recipientAccount);
            if (!recipientAccountInfo) {
                console.log("Recipient account does not exist");
                return false;
            }

            console.log("✓ Claim preconditions validated");
            return true;
        } catch (error) {
            console.log("✗ Claim preconditions validation failed:", error.message);
            return false;
        }
    }

    /**
     * 获取claim事件
     */
    async getClaimEvents(txSignature: string): Promise<any[]> {
        try {
            const tx = await this.provider.connection.getTransaction(txSignature, {
                commitment: 'confirmed',
                maxSupportedTransactionVersion: 0
            });

            if (!tx || !tx.meta || !tx.meta.logMessages) {
                return [];
            }

            // 解析事件日志
            const events = [];
            for (const log of tx.meta.logMessages) {
                if (log.includes('ClaimEvent')) {
                    // 这里需要根据实际的事件格式来解析
                    events.push(log);
                }
            }

            return events;
        } catch (error) {
            console.log("Error getting claim events:", error.message);
            return [];
        }
    }
}