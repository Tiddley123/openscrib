//! Prints what the macOS input hook sees, to check that a tap location really
//! annotates keystrokes with the text they produced:
//!
//!     cargo run -p openscrib-capture --example mac_input_probe
//!
//! Needs Input Monitoring (or Accessibility) for whatever runs it. Type for a
//! few seconds; each event is printed with the text it carried.
fn main() {
    #[cfg(not(target_os = "macos"))]
    println!("macOS only.");

    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEEN: AtomicUsize = AtomicUsize::new(0);

        std::thread::spawn(|| {
            let res = openscrib_capture::mac::input::listen(|e| {
                println!("{:?}  text={:?}", e.event_type, e.name);
                if SEEN.fetch_add(1, Ordering::Relaxed) > 60 {
                    std::process::exit(0);
                }
            });
            if let Err(e) = res {
                eprintln!("the hook could not start: {e}");
                std::process::exit(1);
            }
        });
        println!("watching for 8 seconds; type something...");
        std::thread::sleep(std::time::Duration::from_secs(8));
        println!("{} events seen", SEEN.load(Ordering::Relaxed));
    }
}
