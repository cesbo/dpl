use std::error::Error;

use tracing::error;

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

pub fn exit_with_error(e: &(dyn Error + 'static)) -> ! {
    error!(error = %e, "server failed");
    let mut src = e.source();
    while let Some(s) = src {
        error!(error = %s, "  caused by");
        src = s.source();
    }
    std::process::exit(1);
}
