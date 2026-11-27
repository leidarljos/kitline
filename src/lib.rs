mod emit;
mod gitutil;
mod model;
mod plan;
mod scan;
mod stamp;
mod triage;

const HELP: &str = "\
kitline grades a pile of branches and stamps a change.

  kitline triage <repo> [--trunk main] [--out plan.org] [--keep GLOB]
  kitline triage verify <plan.org>
  kitline stamp [-m TEXT | -F FILE]
  kitline track [REV] [--repo PATH]
  kitline --workspace FILE --report FILE --script FILE

triage writes plan.org. DROP is already upstream. STACKS groups the
rest. ORDER lists them parent before child.
stamp prints a message carrying Kitline-Change. track prints that id.
";

pub fn run() -> i32 {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("triage") => triage::run(),
        Some("stamp") => stamp::run_stamp(),
        Some("track") => stamp::run_track(),
        Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            0
        }
        None => {
            eprint!("{HELP}");
            2
        }
        _ => plan::run(),
    }
}
