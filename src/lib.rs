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
  kitline --version

triage writes plan.org. DROP is already upstream. STACKS groups the
rest. ORDER lists them parent before child.
stamp prints a message carrying Kitline-Change. track prints that id.
";

pub fn run() -> i32 {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else {
        eprint!("{HELP}");
        return 2;
    };
    let Some(first) = first.to_str() else {
        eprintln!("kitline: argument is not utf-8");
        return 1;
    };
    match first {
        "triage" => triage::run(),
        "stamp" => stamp::run_stamp(),
        "track" => stamp::run_track(),
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            0
        }
        "-V" | "--version" => {
            println!("kitline {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => plan::run(),
    }
}
