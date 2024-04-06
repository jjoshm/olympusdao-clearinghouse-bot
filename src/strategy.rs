use crate::{
    bindings::{
        clearinghouse::Clearinghouse,
        cooler::{self, Cooler},
        cooler_factory::{
            ClearRequestFilter, CoolerFactory, DefaultLoanFilter, ExtendLoanFilter, RepayLoanFilter,
        },
    },
    utils::{get_sys_time_in_secs, get_token_price},
};
use anyhow::Result;
use artemis_core::{executors::mempool_executor::SubmitTxToMempool, types::Strategy};
use async_trait::async_trait;
use ethers::{
    contract::parse_log,
    providers::Middleware,
    types::{Address, U256},
};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use std::{fmt::Write, sync::Arc};

use crate::types::{Action, Event};

#[derive(Debug, Clone)]
pub struct LoanTarget<M> {
    pub cooler: cooler::Cooler<M>,
    pub req_id: ::ethers::core::types::U256,
    pub loan_id: ::ethers::core::types::U256,
    pub collateral: ::ethers::core::types::U256,
    pub expiry: ::ethers::core::types::U256,
}

impl<M: Middleware + 'static> LoanTarget<M> {
    pub async fn new(cooler: cooler::Cooler<M>, req_id: U256, loan_id: U256) -> Self {
        let loan = cooler.get_loan(loan_id).await.unwrap();
        Self {
            cooler,
            req_id,
            loan_id,
            collateral: loan.collateral,
            expiry: loan.expiry,
        }
    }

    pub async fn update(&mut self) {
        let loan = self.cooler.get_loan(self.loan_id).await.unwrap();
        self.collateral = loan.collateral;
        self.expiry = loan.expiry;
    }

    pub async fn is_claimable(&self, timestamp: U256) -> bool {
        if self.expiry < timestamp && self.collateral > 0.into() {
            return true;
        } else {
            return false;
        }
    }

    pub async fn calc_rewards(&self, timestamp: U256, ohm_price: U256) -> U256 {
        let elapsed = timestamp - self.expiry;
        let seven_days_in_s: U256 = (7 * 24 * 60 * 60).into();
        let mut max_reward: U256 = (1e17 as u64).into();

        let max_auction_reward = (self.collateral * 5e16 as u64) / 1e18 as u64;
        max_reward = if max_auction_reward < max_reward {
            max_auction_reward
        } else {
            max_reward
        };

        let mut reward_in_gohm: U256 = 0.into();
        if elapsed < seven_days_in_s {
            reward_in_gohm = (max_reward * elapsed) / seven_days_in_s;
        } else {
            reward_in_gohm = max_reward;
        }

        let reward_in_dollar = reward_in_gohm * ohm_price / (1e18 as u64);

        return reward_in_dollar.into();
    }
}

#[derive(Debug, Clone)]
pub struct LiquidationStrategy<M> {
    pub client: Arc<M>,
    pub clearinghouse: Clearinghouse<M>,
    pub cooler_factory: CoolerFactory<M>,
    pub loans: Vec<LoanTarget<M>>,
}

impl<M: Middleware + 'static> LiquidationStrategy<M> {
    pub fn new(
        client: Arc<M>,
        clearinghouse: Clearinghouse<M>,
        cooler_factory: CoolerFactory<M>,
    ) -> Self {
        Self {
            client,
            clearinghouse,
            cooler_factory,
            loans: vec![],
        }
    }
}

impl<M: Middleware + 'static> LiquidationStrategy<M> {
    pub async fn set_loans(&mut self) -> Result<()> {
        println!("Fetching Cooler Loans... ");
        let event: ethers::contract::Event<_, _, _> = self.cooler_factory.clear_request_filter();
        let logs: Vec<ClearRequestFilter> = event.from_block(0).query().await?;
        let logs_len = logs.len();
        let pb = ProgressBar::new(logs_len as u64);
        pb.set_style(
            ProgressStyle::with_template("[{elapsed_precise}] [{wide_bar:.cyan/blue}] ({eta})")
                .unwrap()
                .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
                    let eta = state.eta();
                    let hours = eta.as_secs() / 3600;
                    let minutes = (eta.as_secs() % 3600) / 60;
                    let seconds = eta.as_secs() % 60;
                    write!(w, "{:02}:{:02}:{:02}", hours, minutes, seconds).unwrap()
                })
                .progress_chars("#>-"),
        );
        for log in logs.iter() {
            let cooler = cooler::Cooler::new(log.cooler, self.client.clone());
            let new_loan = LoanTarget::new(cooler, log.req_id, log.loan_id).await;

            self.loans.push(new_loan);
            pb.inc(1);
        }

        pb.finish_and_clear();

        println!("done fetching {} loans.", logs_len);

        Ok(())
    }
}

#[async_trait]
impl<M: Middleware + 'static> Strategy<Event, Action> for LiquidationStrategy<M> {
    async fn sync_state(&mut self) -> Result<()> {
        self.set_loans().await.unwrap();
        println!("Running event loop...");
        Ok(())
    }

    async fn process_event(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::NewBlock(_) => {
                let timestamp = U256::from(get_sys_time_in_secs());
                let ohm_price: U256 =
                    (get_token_price("governance-ohm").await.unwrap() as u64).into();

                let mut total_rewards: U256 = 0.into();
                let mut claimable: (Vec<Address>, Vec<U256>) = (vec![], vec![]);
                for loan in self.loans.iter_mut() {
                    if loan.is_claimable(timestamp).await {
                        loan.update().await;
                        let reward = loan.calc_rewards(timestamp, ohm_price).await;
                        if reward > 0.into() {
                            claimable.0.push(loan.cooler.address());
                            claimable.1.push(loan.loan_id);
                        }
                        total_rewards += reward;
                    }
                }

                if total_rewards <= 0.into() {
                    return vec![];
                }

                let tx = self
                    .clearinghouse
                    .claim_defaulted(claimable.clone().0, claimable.clone().1)
                    .tx;
                let gas_estimate = self.client.estimate_gas(&tx, None).await.unwrap();
                let gas_price = self.client.get_gas_price().await.unwrap();
                let eth_price = get_token_price("ethereum").await.unwrap() as u64;
                let gas_cost_dollar = gas_estimate * gas_price * eth_price / (1e+18 as u64);
                let min_profit: U256 = std::env::var("MIN_PROFIT").unwrap().parse().unwrap();

                if total_rewards > 0.into() {
                    println!(
                        "Found a total of {} claimable cooler loans",
                        claimable.0.len()
                    );
                    println!(
                        "Rewards {}, Gas Cost {}, Min Profit {}",
                        total_rewards.to_string().parse::<f64>().unwrap(),
                        gas_cost_dollar.to_string().parse::<f64>().unwrap(),
                        min_profit.to_string().parse::<f64>().unwrap()
                    );
                    println!("{:?}\n", claimable);
                }

                if total_rewards > gas_cost_dollar && (total_rewards - gas_cost_dollar) > min_profit
                {
                    println!("Claiming rewards for {} loans", claimable.0.len());
                    return vec![Action::SubmitTx(SubmitTxToMempool {
                        tx,
                        gas_bid_info: None,
                    })];
                }

                return vec![];
            }

            Event::NewLoan(log) => {
                let new_loan: ClearRequestFilter = parse_log(log).unwrap();
                let cooler = Cooler::new(new_loan.cooler, self.client.clone());
                println!("[EVENT] New loan created");
                self.loans
                    .push(LoanTarget::new(cooler, new_loan.req_id, new_loan.loan_id).await);
            }

            Event::RepayLoan(log) => {
                let repay_loan: RepayLoanFilter = parse_log(log).unwrap();
                let address = repay_loan.cooler;
                let loan_id = repay_loan.loan_id;

                // update existing loan
                for loan in self.loans.iter_mut() {
                    if loan.loan_id == loan_id && loan.cooler.address() == address {
                        println!("[EVENT] Loan got repayed");
                        loan.update().await;
                    }
                }
            }

            Event::ExtendLoan(log) => {
                let extend_loan: ExtendLoanFilter = parse_log(log).unwrap();
                let address = extend_loan.cooler;
                let loan_id = extend_loan.loan_id;
                for loan in self.loans.iter_mut() {
                    if loan.loan_id == loan_id && loan.cooler.address() == address {
                        println!("[EVENT] Loan got extended");
                        loan.update().await;
                    }
                }
            }

            Event::DefaultLoan(log) => {
                let default_loan: DefaultLoanFilter = parse_log(log).unwrap();
                let address = default_loan.cooler;
                let loan_id = default_loan.loan_id;
                for loan in self.loans.iter_mut() {
                    if loan.loan_id == loan_id && loan.cooler.address() == address {
                        println!("[EVENT] Load got defaulted");
                        loan.update().await;
                    }
                }
            }
        }

        vec![]
    }
}
