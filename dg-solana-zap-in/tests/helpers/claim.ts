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
     * 执行claim操作
     */
    async claim(
        transferId: string,
        params: ClaimParams,
        user: Keypair,
        remainingAccounts: PublicKey[]
    ): Promise<string> {
        const [operationDataPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("operation_data"), Buffer.from(transferId)],
            this.program.programId
        );

        const [registryPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("registry")],
            this.program.programId
        );

        // 计算position NFT mint
        const [positionNftMint] = PublicKey.findProgramAddressSync(
            [Buffer.from("pos_nft_mint"), user.publicKey.toBuffer(), this.poolConfig.poolState.toBuffer()],
            this.program.programId
        );

        // 计算position NFT ATA
        const positionNftAccount = PublicKey.findAssociatedTokenAddressSync(
            user.publicKey,
            positionNftMint
        );

        const tx = await this.program.methods
            .claim(transferId, { minPayout: new this.program.BN(params.minPayout) })
            .accounts({
                operationData: operationDataPda,
                registry: registryPda,
                user: user.publicKey,
                memoProgram: new PublicKey("MemoSq4gqABAX3b7cqRUpy4U5M1ZcvTd5Z73s3J6"),
                clmmProgram: this.poolConfig.clmmProgramId,
                tokenProgram: TOKEN_PROGRAM_ID,
                tokenProgram2022: new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
            })
            .remainingAccounts(remainingAccounts)
            .signers([user])
            .rpc();

        return tx;
    }

    /**
     * 构建claim所需的remaining accounts
     */
    buildRemainingAccounts(
        user: Keypair,
        operationDataPda: PublicKey,
        positionNftMint: PublicKey,
        positionNftAccount: PublicKey,
        recipientTokenAccount: PublicKey
    ): PublicKey[] {
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

            // Tick arrays (需要从外部计算)
            // tickArrayLower,
            // tickArrayUpper,

            // Positions (需要从外部计算)
            // protocolPosition,
            // personalPosition,

            // Operation data PDA
            operationDataPda,

            // PDA token accounts
            // inputTokenAccount,
            // outputTokenAccount,

            // Position NFT
            positionNftAccount,

            // Recipient token account
            recipientTokenAccount,
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
     * 计算PDA token accounts
     */
    calculatePdaTokenAccounts(operationDataPda: PublicKey): [PublicKey, PublicKey] {
        const [inputTokenAccount] = PublicKey.findProgramAddressSync(
            [operationDataPda.toBuffer(), this.poolConfig.tokenMint0.toBuffer()],
            this.program.programId
        );

        const [outputTokenAccount] = PublicKey.findProgramAddressSync(
            [operationDataPda.toBuffer(), this.poolConfig.tokenMint1.toBuffer()],
            this.program.programId
        );

        return [inputTokenAccount, outputTokenAccount];
    }

    /**
     * 完整的claim流程
     */
    async executeClaim(
        transferId: string,
        params: ClaimParams,
        user: Keypair,
        tickLower: number,
        tickUpper: number,
        recipientTokenAccount: PublicKey
    ): Promise<string> {
        console.log("Starting claim process...");

        // 计算所有必要的PDAs
        const [operationDataPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("operation_data"), Buffer.from(transferId)],
            this.program.programId
        );

        const [positionNftMint] = PublicKey.findProgramAddressSync(
            [Buffer.from("pos_nft_mint"), user.publicKey.toBuffer(), this.poolConfig.poolState.toBuffer()],
            this.program.programId
        );

        const positionNftAccount = PublicKey.findAssociatedTokenAddressSync(
            user.publicKey,
            positionNftMint
        );

        const [tickArrayLower, tickArrayUpper] = this.calculateTickArrayPdas(tickLower, tickUpper);
        const [protocolPosition, personalPosition] = this.calculatePositionPdas(tickLower, tickUpper);
        const [inputTokenAccount, outputTokenAccount] = this.calculatePdaTokenAccounts(operationDataPda);

        // 构建remaining accounts
        const remainingAccounts = [
            // Programs
            TOKEN_PROGRAM_ID,
            this.poolConfig.clmmProgramId,
            new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
            new PublicKey("MemoSq4gqABAX3b7cqRUpy4U5M1ZcvTd5Z73s3J6"),
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

            // Operation data PDA
            operationDataPda,

            // PDA token accounts
            inputTokenAccount,
            outputTokenAccount,

            // Position NFT
            positionNftAccount,

            // Recipient token account
            recipientTokenAccount,
        ];

        console.log("Claim accounts prepared, executing...");

        // 执行claim
        const tx = await this.claim(transferId, params, user, remainingAccounts);

        console.log("Claim completed:", tx);
        return tx;
    }
}
