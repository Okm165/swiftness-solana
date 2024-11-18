use clap::Parser;
use futures::{stream::FuturesUnordered, StreamExt};
use serde::Serialize;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::{EncodableKey, Signer},
    system_instruction,
    transaction::Transaction,
};
use std::{path::PathBuf, str::FromStr, thread::sleep, time::Duration};

#[derive(Parser)]
#[command(author, version, about)]
struct SolanaClient {
    /// Path to proof JSON file
    #[clap(short, long)]
    serialized_proof: PathBuf,
}

#[derive(Debug, Serialize)]
struct AllocateRequest {
    idx_start: usize,
    data: Vec<u8>,
}

#[derive(Debug, Serialize)]
enum AllocatorRequest {
    AllocateRequest(AllocateRequest),
    Verify(),
}

// Function to return a Vec of futures for tracking transactions
async fn send_transactions(
    client: &RpcClient,
    transactions: Vec<Transaction>,
) -> Vec<Result<Signature, solana_rpc_client_api::client_error::Error>> {
    let mut futures = FuturesUnordered::new();

    for (idx, tx) in transactions.iter().enumerate() {
        sleep(Duration::from_millis(100));
        // Wrap each transaction in a future and track the result
        let future = async move { (idx, client.send_transaction(tx).await) };
        futures.push(future);
    }

    let mut results = Vec::new();

    while let Some(res) = futures.next().await {
        println!("{:?}", res);
        results.push(res.1)
    }

    results
}

/// Creates a `Transaction` to create an account with rent exemption
async fn create_proof_data_account(
    client: &RpcClient,
    payer: &Keypair,
    proof_data_account: &Keypair,
    proof_size: usize,
    owner: &Pubkey,
) -> Result<Transaction, Box<dyn std::error::Error>> {
    let rent_exemption_amount = client
        .get_minimum_balance_for_rent_exemption(proof_size)
        .await?;

    let create_account_ix = system_instruction::create_account(
        &payer.pubkey(),
        &proof_data_account.pubkey(),
        rent_exemption_amount,
        proof_size as u64,
        owner,
    );

    let blockhash = client.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[create_account_ix],
        Some(&payer.pubkey()),
        &[payer, proof_data_account],
        blockhash,
    );

    Ok(tx)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = SolanaClient::parse();
    let client = RpcClient::new("https://api.devnet.solana.com".to_string());
    let stark_proof = &std::fs::read(cli.serialized_proof)?[..100];
    let verifier_id = Pubkey::from_str("67PTx15uf3UdbSMFb6mwMBxQPE5GeYEyeEsNQa88TJmS").unwrap();
    let payer = Keypair::read_from_file("/home/bartosz/.config/solana/id.json").unwrap();

    let proof_data_account = Keypair::new();
    println!("account pubkey: {:?}", proof_data_account.pubkey());
    client
        .send_and_confirm_transaction(
            &create_proof_data_account(
                &client,
                &payer,
                &proof_data_account,
                stark_proof.len(),
                &verifier_id,
            )
            .await?,
        )
        .await?;

    for (index, chunk) in stark_proof.chunks(10000).enumerate() {
        // Allocate data instructions
        let instructions: Vec<Instruction> = chunk
            .chunks(500)
            .enumerate()
            .map(|(idx, chunk)| {
                let i = idx * 500 + index * 10000;
                Instruction {
                    program_id: verifier_id,
                    accounts: vec![AccountMeta::new(proof_data_account.pubkey(), false)],
                    data: bincode::serialize(&AllocatorRequest::AllocateRequest(AllocateRequest {
                        idx_start: i,
                        data: chunk.to_vec(),
                    }))
                    .unwrap(),
                }
            })
            .collect();

        // Create corresponding transactions
        let block_hash = client.get_latest_blockhash().await?;
        let mut txs = vec![];
        for ix in instructions {
            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&payer.pubkey()),
                &[&payer],
                block_hash,
            );
            txs.push(tx);
        }

        let _results = send_transactions(&client, txs).await;

        sleep(Duration::from_millis(200));
    }

    loop {
        let data = client
            .get_account_data(&proof_data_account.pubkey())
            .await?;

        if data.eq(&stark_proof) {
            println!("proof_data_account correct!");
            break;
        } else {
            println!("proof_data_account data not maching!");
            sleep(Duration::from_secs(5));
        }
    }

    let ix = Instruction {
        program_id: verifier_id,
        accounts: vec![AccountMeta::new(proof_data_account.pubkey(), false)],
        data: bincode::serialize(&AllocatorRequest::Verify()).unwrap(),
    };

    let block_hash = client.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[&payer, &proof_data_account],
        block_hash,
    );

    let result = client.simulate_transaction(&tx).await?;
    println!("Simulation result: {:?}", result);

    // client.send_transaction(&tx).await.unwrap();

    Ok(())
}
