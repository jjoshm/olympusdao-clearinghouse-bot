mod bindings;
mod strategy;
mod types;
mod utils;

use std::sync::Arc;

use crate::bindings::clearinghouse;
use anyhow::Result;
use artemis_core::{
    collectors::{block_collector::BlockCollector, log_collector::LogCollector},
    engine::Engine,
    executors::mempool_executor::MempoolExecutor,
    types::{CollectorMap, ExecutorMap},
};
use bindings::cooler_factory;
use dotenvy::dotenv;
use ethers::{
    middleware::MiddlewareBuilder,
    providers::{Provider, Ws},
    signers::{LocalWallet, Signer},
    types::Address,
};
use strategy::LiquidationStrategy;
use tokio;
use tracing::info;
use types::{Action, Event};
use utils::greet;


#[tokio::main]
async fn main() -> Result<()> {
    greet();
    dotenv().ok();

    let private_key = std::env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set");
    let rpc_provider_read = std::env::var("RPC_PROVIDER_READ").expect("RPC_PROVIDER_READ must be set");
    let rpc_provider_sign = std::env::var("RPC_PROVIDER_SIGN").expect("RPC_PROVIDER_SIGN must be set");
    std::env::var("MIN_PROFIT").expect("MIN_PROFIT must be set");

    let cooler_facrory_address: Address = std::env::var("COOLER_FACTORY_ADDRESS")
        .expect("COOLER_FACTORY_ADDRESS must be set")
        .parse()
        .unwrap();
    let clearinghouse_address: Address = std::env::var("CLEARINGHOUSE_ADDRESS")
        .expect("CLEARINGHOUSE_ADDRESS must be set")
        .parse()
        .unwrap();

    let mut engine: Engine<Event, Action> = Engine::default();

    let ws = Ws::connect(rpc_provider_read).await?;
    let provider_reader = Provider::new(ws);
    let wallet: LocalWallet = private_key.parse().unwrap();
    let address = wallet.address();
    let client_reader = Arc::new(provider_reader.nonce_manager(address).with_signer(wallet.clone()));


    let client_signer = Arc::new((Provider::try_from(rpc_provider_sign)?).with_sender(address).with_signer(wallet));

    let cooler_factory = cooler_factory::CoolerFactory::new(cooler_facrory_address, client_reader.clone());
    let clearinghouse = clearinghouse::Clearinghouse::new(clearinghouse_address, client_reader.clone());
    let strategy = LiquidationStrategy::new(client_reader.clone(), clearinghouse, cooler_factory.clone());

    let new_loan_event = cooler_factory.clear_request_filter();
    let new_loan_collector = LogCollector::new(client_reader.clone(), new_loan_event.filter);
    let new_loan_collector = CollectorMap::new(Box::new(new_loan_collector), Event::NewLoan);

    let repay_loan_event = cooler_factory.repay_loan_filter();
    let repay_loan_collector = LogCollector::new(client_reader.clone(), repay_loan_event.filter);
    let repay_loan_collector = CollectorMap::new(Box::new(repay_loan_collector), Event::RepayLoan);

    let extend_loan_event = cooler_factory.extend_loan_filter();
    let extend_loan_collector = LogCollector::new(client_reader.clone(), extend_loan_event.filter);
    let extend_loan_collector =
        CollectorMap::new(Box::new(extend_loan_collector), Event::ExtendLoan);

    let default_loan_event = cooler_factory.default_loan_filter();
    let default_loan_collector = LogCollector::new(client_reader.clone(), default_loan_event.filter);
    let default_loan_collector =
        CollectorMap::new(Box::new(default_loan_collector), Event::DefaultLoan);

    let block_collector = Box::new(BlockCollector::new(client_reader.clone()));
    let block_collector = CollectorMap::new(block_collector, Event::NewBlock);

    let executor = Box::new(MempoolExecutor::new(client_signer.clone()));
    let executor = ExecutorMap::new(executor, |action| match action {
        Action::SubmitTx(tx) => Some(tx),
    });

    engine.add_collector(Box::new(repay_loan_collector));
    engine.add_collector(Box::new(extend_loan_collector));
    engine.add_collector(Box::new(default_loan_collector));
    engine.add_collector(Box::new(block_collector));
    engine.add_collector(Box::new(new_loan_collector));
    engine.add_strategy(Box::new(strategy));
    engine.add_executor(Box::new(executor));

    if let Ok(mut set) = engine.run().await {
        while let Some(res) = set.join_next().await {
            if res.is_err() {
                return Result::Err(anyhow::Error::msg(res.err().unwrap()));
            } else {
                info!("res: {:?}", res);
            }
        }
    }

    Ok(())
}
