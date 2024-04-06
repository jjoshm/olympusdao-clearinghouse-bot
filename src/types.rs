use artemis_core::{collectors::block_collector::NewBlock, executors::mempool_executor::SubmitTxToMempool};
use ethers::types::Log;

#[derive(Debug, Clone)]
pub enum Event {
    NewBlock(NewBlock),
    NewLoan(Log),
    RepayLoan(Log),
    ExtendLoan(Log),
    DefaultLoan(Log),
}

#[derive(Debug, Clone)]
pub enum Action {
    SubmitTx(SubmitTxToMempool)
}
