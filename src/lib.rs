mod emit;
mod gitutil;
mod model;
mod plan;
mod scan;
mod triage;

pub fn run() -> i32 {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("triage") {
        triage::run()
    } else {
        plan::run()
    }
}
