use clap::ValueEnum;
use strum::Display;

#[derive(Copy, Clone, Display, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum ErrorBehavior {
    Ignore,
    Display,
    Stop,
}
