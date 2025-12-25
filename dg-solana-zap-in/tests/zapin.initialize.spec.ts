import * as anchor from "@coral-xyz/anchor";
import { PublicKey, SystemProgram } from "@solana/web3.js";
import {Program} from "@coral-xyz/anchor";

// PDA: seeds = [b"operation_data"]
function operationDataSeedPda(programId: PublicKey) {
    return PublicKey.findProgramAddressSync(
        [Buffer.from("operation_data")],
        programId
    );
}

describe("dg_solana_zapin :: initialize", () => {
    const connection = new anchor.web3.Connection("https://warmhearted-delicate-uranium.solana-devnet.quiknode.pro/300dfad121b027e64f41fc3b31d342d4b38ed5be");
    const wallet = anchor.Wallet.local();
    const provider = new anchor.AnchorProvider(connection, wallet, anchor.AnchorProvider.defaultOptions());
    anchor.setProvider(provider);

    const program = anchor.workspace.dgSolanaZapin as Program<dgSolanaZapin>;

    // Use the provider wallet as authority (works on any RPC, no airdrop needed)
    const authorityPubkey = provider.wallet.publicKey;
    console.log("provider wallet:", authorityPubkey.toBase58())
    console.log("program ID:", program.programId.toBase58())

    it("creates operation_data PDA and marks it initialized", async () => {
        const [opPda] = operationDataSeedPda(program.programId);
        console.log("operation data pubkey: ", opPda.toBase58())

        await program.methods
            .initialize()
            .accounts({
                operationData: opPda,
                authority: authorityPubkey,
                systemProgram: SystemProgram.programId,
            })
            .rpc();

        const od = await program.account.operationData.fetch(opPda);

        if (!od.initialized) throw new Error("operation_data not initialized");
        if (!od.authority.equals(authorityPubkey)) {
            throw new Error(`authority mismatch: got ${od.authority.toBase58()}`);
        }
    }).timeout(60_000);
});