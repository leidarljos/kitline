mod emit;
mod gitutil;
mod model;
mod plan;
mod scan;

pub fn run() -> i32 {
    plan::run()
}
