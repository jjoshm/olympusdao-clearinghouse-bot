use crate::{
    bindings::{
        clearinghouse::{ClaimDefaultedCall, Clearinghouse},
        cooler::Cooler,
        cooler_factory::{
            ClearRequestFilter, CoolerFactory, DefaultLoanFilter, ExtendLoanFilter, RepayLoanFilter,
        },
    },
    utils::{get_sys_time_in_secs, get_token_price, greet},
};
use anyhow::Result;
use artemis_core::{executors::mempool_executor::SubmitTxToMempool, types::Strategy};
use async_trait::async_trait;
use comfy_table::{presets::UTF8_FULL, Attribute, Cell, Color, Table};
use ethers::{contract::parse_log, providers::Middleware, types::U256};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use std::{fmt::Write, sync::Arc};

use crate::types::{Action, Event};

use chrono::{DateTime, TimeZone, Utc};

#[derive(Debug, Clone)]
pub struct LoanTarget<M> {
    pub cooler: Cooler<M>,
    pub req_id: U256,
    pub loan_id: U256,
    pub collateral: U256,
    pub expiry: U256,
}

#[derive(Debug)]
pub struct LiquidationStrategy<M> {
    pub client: Arc<M>,
    pub clearinghouse: Clearinghouse<M>,
    pub cooler_factory: CoolerFactory<M>,
    pub loans: Vec<LoanTarget<M>>,
}

impl<M: Middleware + 'static> LoanTarget<M> {
    pub async fn new(cooler: Cooler<M>, req_id: U256, loan_id: U256) -> Self {
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

    pub fn is_claimable(&self, timestamp: U256) -> bool {
        if self.expiry < timestamp && self.collateral > 0.into() {
            return true;
        } else {
            return false;
        }
    }

    pub fn calc_reward_percentage(&self) -> U256 {
        let timestamp = U256::from(get_sys_time_in_secs());
        let elapsed = timestamp - self.expiry;
        let seven_days_in_s: U256 = (7 * 24 * 60 * 60).into();
        let reward_percentage = if elapsed < seven_days_in_s {
            (elapsed * 100) / seven_days_in_s
        } else {
            100.into()
        };

        return reward_percentage;
    }

    pub fn calc_rewards_in_dollar(&self, timestamp: U256, ohm_price: U256) -> U256 {
        let elapsed = timestamp - self.expiry;
        let seven_days_in_s: U256 = (7 * 24 * 60 * 60).into();
        let mut max_reward: U256 = (1e17 as u64).into();

        let max_auction_reward = (self.collateral * 5e16 as u64) / 1e18 as u64;
        max_reward = if max_auction_reward < max_reward {
            max_auction_reward
        } else {
            max_reward
        };

        let reward_in_gohm: U256 = if elapsed < seven_days_in_s {
            (max_reward * elapsed) / seven_days_in_s
        } else {
            max_reward
        };

        let reward_in_dollar = reward_in_gohm * ohm_price / (1e18 as u64);

        return reward_in_dollar.into();
    }
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
    async fn print_table(&self, claimable: U256, gohm_price: U256) {
        println!("\x1B[2J\x1B[1;1H");
        greet();

        let mut table_info = Table::new();
        let ohm_price = get_token_price("governance-ohm").await.unwrap() as u64;
        let expired_loans: Vec<&LoanTarget<M>> = self
            .loans
            .iter()
            .filter(|loan| {
                loan.expiry < U256::from(get_sys_time_in_secs())
                    && loan.collateral > 0.into()
                    && loan.calc_rewards_in_dollar(
                        U256::from(get_sys_time_in_secs()),
                        ohm_price.into(),
                    ) > 0.into()
            })
            .collect();

        let total_collateral_gohm = expired_loans
            .iter()
            .fold(U256::from(0), |acc, loan| acc + loan.collateral)
            / (1e18 as u64) as u64;

        let timestamp = U256::from(get_sys_time_in_secs());
        let next_expiry = self.loans.iter().fold(U256::MAX, |acc, loan| {
            if loan.expiry > timestamp && loan.expiry < acc {
                loan.expiry - timestamp
            } else {
                acc
            }
        });

        let claiable_consider_gas_and_targets =
            self.loans.iter().filter(|loan| {
                loan.is_claimable(U256::from(get_sys_time_in_secs()))
                    && loan.calc_rewards_in_dollar(
                        U256::from(get_sys_time_in_secs()),
                        ohm_price.into(),
                    ) > 0.into()
            })
            .fold(U256::from(0), |acc, loan| {
                if loan.calc_reward_percentage()
                    > std::env::var("REWARD_PERIOD_TARGET")
                        .unwrap()
                        .parse()
                        .unwrap()
                {
                    return acc + loan.calc_rewards_in_dollar(
                        U256::from(get_sys_time_in_secs()),
                        ohm_price.into(),
                    )
                } else {
                    return acc
                }
            });

        let claiable_consider_gas_and_targets: U256 =
            if claiable_consider_gas_and_targets < std::env::var("MIN_PROFIT").unwrap().parse().unwrap() {
                0.into()
            } else {
                claiable_consider_gas_and_targets.into()
            };

        table_info.load_preset(UTF8_FULL).set_header(vec![
            "Claimable",
            "Claimable inc. gas and target",
            "Profit Target",
            "Reward Period Target",
            "Expired Loans",
            "Total Collateral",
            "Next Expiry",
        ]);

        let duration: DateTime<Utc> = Utc.timestamp_opt(next_expiry.as_u64() as i64, 0).unwrap();
        let duration = duration.format("%Hh:%Mm:%Ss");
        table_info.load_preset(UTF8_FULL).add_row(vec![
            format!("{} dollar", claimable.to_string()),
            format!("{} dollar", claiable_consider_gas_and_targets.to_string()),
            format!("{} dollar", std::env::var("MIN_PROFIT").unwrap()),
            format!("{}%", std::env::var("REWARD_PERIOD_TARGET").unwrap()),
            expired_loans.len().to_string(),
            format!("{} gOHM", total_collateral_gohm.to_string()),
            format!("{}", duration),
        ]);

        let mut table_loans = Table::new();
        table_loans.load_preset(UTF8_FULL).set_header(vec![
            "Cooler",
            "Loan ID",
            "Collateral",
            "Expire time (UTC)",
            "Reward period passed",
            "Reward",
        ]);
        for loan in expired_loans.iter() {
            let is_reward_period_target_hit = loan.calc_reward_percentage()
                > std::env::var("REWARD_PERIOD_TARGET")
                    .unwrap()
                    .parse()
                    .unwrap();
            let reward_target_text = format!("{}%", loan.calc_reward_percentage());
            let reward_target_text: Cell = if is_reward_period_target_hit {
                Cell::new(reward_target_text)
                    .fg(Color::Green)
                    .add_attributes(vec![Attribute::Bold])
            } else {
                Cell::new(reward_target_text)
            };

            let readable_expiry = chrono::Utc
                .timestamp_opt(loan.expiry.as_u64() as i64, 0)
                .unwrap();
            let readable_expiry = readable_expiry.format("%Y-%m-%d %H:%M:%S").to_string();
            table_loans.load_preset(UTF8_FULL).add_row(vec![
                Cell::new(loan.cooler.address().to_string()),
                Cell::new(loan.loan_id.to_string()),
                Cell::new(loan.collateral.to_string()),
                Cell::new(readable_expiry),
                reward_target_text,
                Cell::new(format!(
                    "{} dollar",
                    loan.calc_rewards_in_dollar(
                        U256::from(get_sys_time_in_secs()),
                        gohm_price.into(),
                    )
                    .to_string(),
                )),
            ]);
        }

        println!();
        println!("{}", table_info);

        if expired_loans.len() > 0 {
            println!();
            println!("{}", table_loans);
        }
    }
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
            let cooler = Cooler::new(log.cooler, self.client.clone());
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
                let gohm_price = get_token_price("governance-ohm").await.unwrap() as u64;
                let mut claimable_loans = self
                    .loans
                    .iter_mut()
                    .filter(|loan| {
                        loan.is_claimable(U256::from(get_sys_time_in_secs()))
                            && loan.calc_rewards_in_dollar(
                                U256::from(get_sys_time_in_secs()),
                                gohm_price.into(),
                            ) > 0.into()
                    })
                    .collect::<Vec<&mut LoanTarget<M>>>();

                let claimable_dollar_raw =
                    claimable_loans.iter_mut().fold(U256::from(0), |acc, loan| {
                        acc + loan.calc_rewards_in_dollar(
                            U256::from(get_sys_time_in_secs()),
                            gohm_price.into(),
                        )
                    });

                let mut claimable_loans_with_reward_limit_hit = claimable_loans
                    .iter_mut()
                    .filter(|loan| {
                        loan.calc_reward_percentage()
                            > std::env::var("REWARD_PERIOD_TARGET")
                                .unwrap()
                                .parse()
                                .unwrap()
                    })
                    .collect::<Vec<&mut &mut LoanTarget<M>>>();

                for loan in claimable_loans_with_reward_limit_hit.iter_mut() {
                    loan.update().await;
                }

                if claimable_loans_with_reward_limit_hit.len() == 0 {
                    self.print_table(claimable_dollar_raw, gohm_price.into()).await;
                    return vec![];
                }

                let claimable_reward_hit_dollar = claimable_loans_with_reward_limit_hit
                    .iter()
                    .fold(U256::from(0), |acc, loan| {
                        acc + loan.calc_rewards_in_dollar(
                            U256::from(get_sys_time_in_secs()),
                            gohm_price.into(),
                        )
                    });

                let claim_default_arguments: ClaimDefaultedCall =
                    claimable_loans_with_reward_limit_hit.iter().fold(
                        ClaimDefaultedCall {
                            loans: vec![],
                            coolers: vec![],
                        },
                        |mut acc, loan| {
                            acc.loans.push(loan.loan_id);
                            acc.coolers.push(loan.cooler.address());
                            acc
                        },
                    );

                let gas_price = self.client.get_gas_price().await.unwrap();
                let tx = self
                    .clearinghouse
                    .claim_defaulted(
                        claim_default_arguments.coolers,
                        claim_default_arguments.loans,
                    )
                    .tx;

                let gas_estimate = self.client.estimate_gas(&tx, None).await.unwrap();
                let gas_cost = gas_estimate * gas_price;
                let gas_cost_dollar =
                    gas_cost * get_token_price("ethereum").await.unwrap() as u64 / 1e18 as u64;
                let net_claimable_reward_target_hit_dollar =
                    claimable_reward_hit_dollar - gas_cost_dollar;
                let profit_target_hit = net_claimable_reward_target_hit_dollar
                    > std::env::var("MIN_PROFIT").unwrap().parse().unwrap();

                self.print_table(claimable_dollar_raw, gohm_price.into()).await;

                if profit_target_hit {
                    println!("[ACTION] Claiming loans...");
                    return vec![Action::SubmitTx(SubmitTxToMempool {
                        tx,
                        gas_bid_info: None,
                    })];
                }
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
