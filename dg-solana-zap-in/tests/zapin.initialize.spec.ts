import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, SystemProgram } from "@solana/web3.js";
import { dgSolanaZapin } from "../target/types/dg_solana_zapin";

// PDA: seeds = [b"operation_data"]
function operationDataSeedPda(programId: PublicKey) {
    return PublicKey.findProgramAddressSync(
        [Buffer.from("operation_data")],
        programId
    );
}

describe("dg_solana_zapin :: initialize", () => {
    const provider = anchor.AnchorProvider.env();
    anchor.setProvider(provider);

    const program = anchor.workspace.dg_solana_zapin as Program<dgSolanaZapin>;

    // Use the provider wallet as authority (works on any RPC, no airdrop needed)
    const authorityPubkey = provider.wallet.publicKey;
    console.log("provider wallet:", authorityPubkey.toBase58())
    console.log("program ID:", program.programId.toBase58())

    it("creates operation_data PDA and marks it initialized", async () => {
        const [opPda] = operationDataSeedPda(program.programId);

        await program.methods
            .initialize()
            .accounts({
                operationData: opPda,
                authority: authorityPubkey,
                systemProgram: SystemProgram.programId,
            })
            // no extra signers needed; provider.wallet signs
            .rpc();

        const od = await program.account.operationData.fetch(opPda);

        if (!od.initialized) throw new Error("operation_data not initialized");
        if (!od.authority.equals(authorityPubkey)) {
            throw new Error(`authority mismatch: got ${od.authority.toBase58()}`);
        }
    }).timeout(60_000);
});