use std::error::Error;

pub fn format_error_chain(e: &(dyn Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        out.push_str(": ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

pub fn exit_with_stderr(e: &(dyn Error + 'static)) -> ! {
    eprintln!("error: {e}");
    let mut src = e.source();
    while let Some(s) = src {
        eprintln!("  caused by: {s}");
        src = s.source();
    }
    std::process::exit(1);
}
