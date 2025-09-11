import * as anchor from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { 
    getOrCreateAssociatedTokenAccount, 
    getAssociatedTokenAddress,
    TOKEN_PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID
} from "@solana/spl-token";
import { 
    operationDataPda, 
    positionNftMintPda, 
    tickArrayPda, 
    protocolPositionPda,
    tickArrayStartIndex 
} from "./pdas";
import { encodeZapInParams, OperationType } from "./params";
import { createUserWithSol, createMintAndATA, getTokenAmount } from "./token";
import { ClaimClient, ClaimConfig, ClaimParams } from "./claim";

export interface ZapInConfig {
    program: anchor.Program;
    provider: anchor.AnchorProvider;
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

export interface ZapInParams {
    amountIn: anchor.BN;
    tickLower: number;
    tickUpper: number;
    slippageBps: number;
}

export class ZapInClient {
    private program: anchor.Program;
    private provider: anchor.AnchorProvider;
    private poolConfig: ZapInConfig['poolConfig'];

    constructor(config: ZapInConfig) {
        this.program = config.program;
        this.provider = config.provider;
        this.poolConfig = config.poolConfig;
    }

    /**
     * 生成32字节的transfer_id
     */
    generateTransferId(): [number; 32] {
        const keypair = Keypair.generate();
        return Array.from(keypair.publicKey.toBytes()) as [number; 32];
    }

    /**
     * 步骤1: 初始化操作数据PDA
     */
    async initialize(): Promise<PublicKey> {
        const [operationDataPda] = operationDataPda("", this.program.programId);
        
        const tx = await this.program.methods
            .initialize()
            .accounts({
                operationData: operationDataPda,
                authority: this.provider.wallet.publicKey,
                systemProgram: SystemProgram.programId,
            })
            .rpc();

        console.log("Initialize transaction:", tx);
        return operationDataPda;
    }

    /**
     * 步骤2: 存入资金和操作参数
     */
    async deposit(
        transferId: [number; 32],
        params: ZapInParams,
        user: Keypair,
        userAta: PublicKey
    ): Promise<string> {
        const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), this.program.programId);
        const [registryPda] = operationDataPda("registry", this.program.programId);

        // 创建程序拥有的代币账户
        const programTokenAccount = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint0, // 假设使用token0作为输入
            operationDataPda,
            true
        );

        // 编码ZapIn参数
        const action = encodeZapInParams(this.program, {
            amountIn: params.amountIn,
            pool: this.poolConfig.poolState,
            tickLower: params.tickLower,
            tickUpper: params.tickUpper,
            slippageBps: params.slippageBps
        });

        const tx = await this.program.methods
            .deposit(
                transferId,
                OperationType.ZapIn,
                action,
                params.amountIn,
                this.poolConfig.poolState, // ca
                user.publicKey // authorized_executor
            )
            .accounts({
                registry: registryPda,
                operationData: operationDataPda,
                authority: user.publicKey,
                authorityAta: userAta,
                programTokenAccount: programTokenAccount.address,
                clmmProgram: this.poolConfig.clmmProgramId,
                poolState: this.poolConfig.poolState,
                ammConfig: this.poolConfig.ammConfig,
                observationState: this.poolConfig.observationState,
                tokenVault0: this.poolConfig.tokenVault0,
                tokenVault1: this.poolConfig.tokenVault1,
                tokenMint0: this.poolConfig.tokenMint0,
                tokenMint1: this.poolConfig.tokenMint1,
                tokenProgram: TOKEN_PROGRAM_ID,
                systemProgram: SystemProgram.programId,
            })
            .signers([user])
            .rpc();

        console.log("Deposit transaction:", tx);
        return tx;
    }

    /**
     * 步骤3: 准备执行 - 转移资金到PDA账户
     */
    async prepareExecute(
        transferId: [number; 32],
        user: Keypair,
        refundAta: PublicKey
    ): Promise<string> {
        const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), this.program.programId);

        // 创建PDA代币账户
        const pdaToken0 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint0,
            operationDataPda,
            true
        );

        const pdaToken1 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint1,
            operationDataPda,
            true
        );

        // 创建程序代币账户
        const programTokenAccount = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint0,
            operationDataPda,
            true
        );

        const tx = await this.program.methods
            .prepareExecute(transferId)
            .accounts({
                operationData: operationDataPda,
                user: user.publicKey,
                programTokenAccount: programTokenAccount.address,
                refundAta: refundAta,
                pdaToken0: pdaToken0.address,
                pdaToken1: pdaToken1.address,
                poolState: this.poolConfig.poolState,
                ammConfig: this.poolConfig.ammConfig,
                observationState: this.poolConfig.observationState,
                tokenVault0: this.poolConfig.tokenVault0,
                tokenVault1: this.poolConfig.tokenVault1,
                tokenMint0: this.poolConfig.tokenMint0,
                tokenMint1: this.poolConfig.tokenMint1,
                tokenProgram: TOKEN_PROGRAM_ID,
                systemProgram: SystemProgram.programId,
                rent: anchor.web3.SYSVAR_RENT_PUBKEY,
            })
            .signers([user])
            .rpc();

        console.log("Prepare execute transaction:", tx);
        return tx;
    }

    /**
     * 步骤4: 执行swap操作平衡代币
     */
    async swapForBalance(
        transferId: [number; 32],
        user: Keypair
    ): Promise<string> {
        const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), this.program.programId);

        // 创建PDA代币账户
        const pdaToken0 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint0,
            operationDataPda,
            true
        );

        const pdaToken1 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint1,
            operationDataPda,
            true
        );

        const tx = await this.program.methods
            .swapForBalance(transferId)
            .accounts({
                operationData: operationDataPda,
                user: user.publicKey,
                clmmProgram: this.poolConfig.clmmProgramId,
                poolState: this.poolConfig.poolState,
                ammConfig: this.poolConfig.ammConfig,
                observationState: this.poolConfig.observationState,
                tokenMint0: this.poolConfig.tokenMint0,
                tokenMint1: this.poolConfig.tokenMint1,
                pdaToken0: pdaToken0.address,
                pdaToken1: pdaToken1.address,
                tokenVault0: this.poolConfig.tokenVault0,
                tokenVault1: this.poolConfig.tokenVault1,
                tokenProgram: TOKEN_PROGRAM_ID,
                tokenProgram2022: TOKEN_2022_PROGRAM_ID,
                memoProgram: new PublicKey("MemoSq4gqABAX3b7cqRUpy4U5M1ZcvTd5Z73s3J6"), // Memo program
            })
            .signers([user])
            .rpc();

        console.log("Swap for balance transaction:", tx);
        return tx;
    }

    /**
     * 步骤5: 打开流动性仓位
     */
    async openPosition(
        transferId: [number; 32],
        user: Keypair
    ): Promise<string> {
        const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), this.program.programId);
        const [positionNftMint] = positionNftMintPda(
            user.publicKey,
            this.poolConfig.poolState,
            this.program.programId
        );

        // 创建PDA代币账户
        const pdaToken0 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint0,
            operationDataPda,
            true
        );

        const pdaToken1 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint1,
            operationDataPda,
            true
        );

        // 计算tick array PDAs
        const lowerStart = tickArrayStartIndex(-120, this.poolConfig.tickSpacing);
        const upperStart = tickArrayStartIndex(120, this.poolConfig.tickSpacing);
        
        const [tickArrayLower] = tickArrayPda(
            this.poolConfig.poolState,
            lowerStart,
            this.poolConfig.clmmProgramId
        );

        const [tickArrayUpper] = tickArrayPda(
            this.poolConfig.poolState,
            upperStart,
            this.poolConfig.clmmProgramId
        );

        const [protocolPosition] = protocolPositionPda(
            this.poolConfig.poolState,
            lowerStart,
            upperStart,
            this.poolConfig.clmmProgramId
        );

        // 创建用户的位置NFT账户
        const positionNftAccount = await getAssociatedTokenAddress(
            positionNftMint,
            user.publicKey
        );

        // 计算metadata PDA
        const metadataProgramId = new PublicKey("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
        const [metadataAccount] = PublicKey.findProgramAddressSync(
            [
                Buffer.from("metadata"),
                metadataProgramId.toBuffer(),
                positionNftMint.toBuffer()
            ],
            metadataProgramId
        );

        const tx = await this.program.methods
            .openPositionStep(transferId)
            .accounts({
                operationData: operationDataPda,
                user: user.publicKey,
                clmmProgram: this.poolConfig.clmmProgramId,
                poolState: this.poolConfig.poolState,
                tickArrayLower: tickArrayLower,
                tickArrayUpper: tickArrayUpper,
                protocolPosition: protocolPosition,
                personalPosition: Keypair.generate().publicKey, // 临时，实际应该从operation_data读取
                positionNftMint: positionNftMint,
                positionNftAccount: positionNftAccount,
                tokenMint0: this.poolConfig.tokenMint0,
                tokenMint1: this.poolConfig.tokenMint1,
                tokenVault0: this.poolConfig.tokenVault0,
                tokenVault1: this.poolConfig.tokenVault1,
                pdaToken0: pdaToken0.address,
                pdaToken1: pdaToken1.address,
                tokenProgram: TOKEN_PROGRAM_ID,
                tokenProgram2022: TOKEN_2022_PROGRAM_ID,
                systemProgram: SystemProgram.programId,
                rent: anchor.web3.SYSVAR_RENT_PUBKEY,
                associatedTokenProgram: anchor.utils.token.ASSOCIATED_PROGRAM_ID,
                metadataProgram: metadataProgramId,
                metadataAccount: metadataAccount,
            })
            .signers([user])
            .rpc();

        console.log("Open position transaction:", tx);
        return tx;
    }

    /**
     * 步骤6: 增加流动性
     */
    async increaseLiquidity(
        transferId: [number; 32],
        user: Keypair
    ): Promise<string> {
        const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), this.program.programId);

        // 创建PDA代币账户
        const pdaToken0 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint0,
            operationDataPda,
            true
        );

        const pdaToken1 = await getOrCreateAssociatedTokenAccount(
            this.provider.connection,
            user,
            this.poolConfig.tokenMint1,
            operationDataPda,
            true
        );

        // 计算tick array PDAs
        const lowerStart = tickArrayStartIndex(-120, this.poolConfig.tickSpacing);
        const upperStart = tickArrayStartIndex(120, this.poolConfig.tickSpacing);
        
        const [tickArrayLower] = tickArrayPda(
            this.poolConfig.poolState,
            lowerStart,
            this.poolConfig.clmmProgramId
        );

        const [tickArrayUpper] = tickArrayPda(
            this.poolConfig.poolState,
            upperStart,
            this.poolConfig.clmmProgramId
        );

        const [protocolPosition] = protocolPositionPda(
            this.poolConfig.poolState,
            lowerStart,
            upperStart,
            this.poolConfig.clmmProgramId
        );

        // 创建用户的位置NFT账户
        const [positionNftMint] = positionNftMintPda(
            user.publicKey,
            this.poolConfig.poolState,
            this.program.programId
        );

        const positionNftAccount = await getAssociatedTokenAddress(
            positionNftMint,
            user.publicKey
        );

        const tx = await this.program.methods
            .increaseLiquidityStep(transferId)
            .accounts({
                operationData: operationDataPda,
                user: user.publicKey,
                clmmProgram: this.poolConfig.clmmProgramId,
                poolState: this.poolConfig.poolState,
                protocolPosition: protocolPosition,
                personalPosition: Keypair.generate().publicKey, // 临时，实际应该从operation_data读取
                tickArrayLower: tickArrayLower,
                tickArrayUpper: tickArrayUpper,
                pdaToken0: pdaToken0.address,
                pdaToken1: pdaToken1.address,
                tokenVault0: this.poolConfig.tokenVault0,
                tokenVault1: this.poolConfig.tokenVault1,
                tokenMint0: this.poolConfig.tokenMint0,
                tokenMint1: this.poolConfig.tokenMint1,
                positionNftAccount: positionNftAccount,
                tokenProgram: TOKEN_PROGRAM_ID,
                tokenProgram2022: TOKEN_2022_PROGRAM_ID,
            })
            .signers([user])
            .rpc();

        console.log("Increase liquidity transaction:", tx);
        return tx;
    }

    /**
     * 步骤7: 完成执行
     */
    async finalizeExecute(
        transferId: [number; 32],
        user: Keypair
    ): Promise<string> {
        const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), this.program.programId);

        const tx = await this.program.methods
            .finalizeExecute(transferId)
            .accounts({
                operationData: operationDataPda,
                user: user.publicKey,
            })
            .signers([user])
            .rpc();

        console.log("Finalize execute transaction:", tx);
        return tx;
    }

    /**
     * 取消操作
     */
    async cancel(
        transferId: [number; 32],
        user: Keypair
    ): Promise<string> {
        const [operationDataPda] = operationDataPda(Buffer.from(transferId).toString('hex'), this.program.programId);

        const tx = await this.program.methods
            .cancel(transferId)
            .accounts({
                operationData: operationDataPda,
                user: user.publicKey,
            })
            .signers([user])
            .rpc();

        console.log("Cancel transaction:", tx);
        return tx;
    }

    /**
     * 执行完整的zap-in流程
     */
    async executeZapIn(
        params: ZapInParams,
        user: Keypair
    ): Promise<{
        transferId: [number; 32];
        transactions: string[];
    }> {
        const transferId = this.generateTransferId();
        const transactions: string[] = [];

        try {
            // 创建用户代币账户
            const userAta = await getOrCreateAssociatedTokenAccount(
                this.provider.connection,
                user,
                this.poolConfig.tokenMint0,
                user.publicKey
            );

            const refundAta = await getOrCreateAssociatedTokenAccount(
                this.provider.connection,
                user,
                this.poolConfig.tokenMint0,
                user.publicKey
            );

            // 步骤1: 初始化
            await this.initialize();
            transactions.push("initialize");

            // 步骤2: 存入资金
            const depositTx = await this.deposit(transferId, params, user, userAta.address);
            transactions.push(depositTx);

            // 步骤3: 准备执行
            const prepareTx = await this.prepareExecute(transferId, user, refundAta.address);
            transactions.push(prepareTx);

            // 步骤4: 执行swap
            const swapTx = await this.swapForBalance(transferId, user);
            transactions.push(swapTx);

            // 步骤5: 打开仓位
            const openTx = await this.openPosition(transferId, user);
            transactions.push(openTx);

            // 步骤6: 增加流动性
            const liquidityTx = await this.increaseLiquidity(transferId, user);
            transactions.push(liquidityTx);

            // 步骤7: 完成执行
            const finalizeTx = await this.finalizeExecute(transferId, user);
            transactions.push(finalizeTx);

            return { transferId, transactions };

        } catch (error) {
            console.error("Zap-in execution failed:", error);
            // 可以在这里添加取消逻辑
            throw error;
        }
    }

    /**
     * 创建claim客户端
     */
    createClaimClient(): ClaimClient {
        const claimConfig: ClaimConfig = {
            program: this.program,
            provider: this.provider,
            poolConfig: this.poolConfig
        };
        return new ClaimClient(claimConfig);
    }

    /**
     * 执行claim操作
     */
    async claim(
        transferId: string,
        params: ClaimParams,
        user: Keypair,
        tickLower: number,
        tickUpper: number,
        recipientTokenAccount: PublicKey
    ): Promise<string> {
        const claimClient = this.createClaimClient();
        return await claimClient.executeClaim(
            transferId,
            params,
            user,
            tickLower,
            tickUpper,
            recipientTokenAccount
        );
    }
}
