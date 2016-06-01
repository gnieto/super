#[macro_use]
extern crate clap;
extern crate colored;
extern crate zip;
extern crate xml;
extern crate serde;
extern crate serde_json;
extern crate chrono;
extern crate toml;
extern crate regex;
extern crate crypto;
extern crate rustc_serialize;

mod decompilation;
mod static_analysis;
mod results;
mod config;

use std::{fs, io, fmt, result};
use std::path::Path;
use std::fmt::Display;
use std::str::FromStr;
use std::error::Error as StdError;
use std::io::Write;
use std::process::exit;
use std::time::Instant;

use serde::ser::{Serialize, Serializer};
use serde_json::error::ErrorCode as JSONErrorCode;
use clap::{Arg, App, ArgMatches};
use colored::Colorize;

use decompilation::*;
use static_analysis::*;
use results::*;
pub use config::Config;

fn main() {
    let matches = get_help_menu();

    let app_id = matches.value_of("package").unwrap();
    let verbose = matches.is_present("verbose");
    let quiet = matches.is_present("quiet");
    let force = matches.is_present("force");
    let bench = matches.is_present("bench");
    let config = match Config::new(app_id, verbose, quiet, force, bench) {
        Ok(c) => c,
        Err(e) => {
            print_warning(format!("There was an error when reading the config.toml file: {}",
                                  e),
                          verbose);
            let mut c: Config = Default::default();
            c.set_app_id(app_id);
            c.set_verbose(verbose);
            c.set_quiet(quiet);
            c.set_force(force);
            c.set_bench(bench);
            c
        }
    };

    if !config.check() {
        print_error(format!("There is an error with the configuration: {:?}", config),
                    verbose);
        exit(Error::Config.into());
    }

    if config.is_verbose() {
        println!("Welcome to the Android Anti-Rebelation project. We will now try to audit the \
                  given application.");
        println!("You activated the verbose mode. {}",
                 "May Tux be with you!".bold());
        println!("");
    }

    let mut benches = Vec::with_capacity(3);

    let start_time = Instant::now();

    // APKTool app decompression
    decompress(&config);

    if config.is_bench() {
        benches.push(Benchmark::new("ApkTool decompression", start_time.elapsed()));
    }

    let dex_start = Instant::now();

    // Extracting the classes.dex from the .apk file
    extract_dex(&config);

    if config.is_bench() {
        benches.push(Benchmark::new("Dex extraction", dex_start.elapsed()));
    }

    if config.is_verbose() {
        println!("");
        println!("Now it's time for the actual decompilation of the source code. We'll translate \
                  Android JVM bytecode to Java, so that we can check the code afterwards.");
    }

    let decompile_start = Instant::now();

    // Decompiling the app
    decompile(&config);

    if config.is_bench() {
        benches.push(Benchmark::new("Decompilation", decompile_start.elapsed()));
    }

    if let Some(mut results) = Results::init(&config) {
        if config.is_bench() {
            while benches.len() > 0 {
                results.add_benchmark(benches.remove(0));
            }
        }

        let static_start = Instant::now();
        // Static application analysis
        static_analysis(&config, &mut results);

        if config.is_bench() {
            results.add_benchmark(Benchmark::new("Static analysis", static_start.elapsed()));
        }

        // TODO dynamic analysis

        if !config.is_quiet() {
            println!("");
        }

        let report_start = Instant::now();

        match results.generate_report(&config) {
            Ok(_) => {
                if config.is_verbose() {
                    println!("The results report has been saved. Everything went smoothly, now \
                              you can check all the results.");
                    println!("");
                    println!("I will now analyze myself for vulnerabilities…");
                    println!("Nah, just kidding, I've been developed in {}!",
                             "Rust".bold().green())
                } else if !config.is_quiet() {
                    println!("Report generated.");
                }
            }
            Err(e) => {
                print_error(format!("There was an error generating the results report: {}", e),
                            config.is_verbose());
                exit(Error::Unknown.into())
            }
        }

        if config.is_bench() {
            results.add_benchmark(Benchmark::new("Report generation", report_start.elapsed()));
        }

        if config.is_bench() {
            results.add_benchmark(Benchmark::new("Total time", start_time.elapsed()));
            println!("");
            println!("{}", "Benchmarks:".bold());
            for bench in results.get_benchmarks() {
                println!("{}", bench);
            }
        }
    } else if !config.is_quiet() {
        println!("Analysis cancelled.");
    }
}

fn file_exists<P: AsRef<Path>>(path: P) -> bool {
    fs::metadata(path).is_ok()
}

#[derive(Debug)]
pub enum Error {
    AppNotExists,
    ParseError,
    JSONError(JSONError),
    CodeNotFound,
    Config,
    IOError(io::Error),
    Unknown,
}

impl Into<i32> for Error {
    fn into(self) -> i32 {
        match self {
            Error::AppNotExists => 10,
            Error::ParseError => 20,
            Error::JSONError(_) => 30,
            Error::CodeNotFound => 40,
            Error::Config => 50,
            Error::IOError(_) => 100,
            Error::Unknown => 1,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::IOError(err)
    }
}

impl From<serde_json::error::Error> for Error {
    fn from(err: serde_json::error::Error) -> Error {
        match err {
            serde_json::error::Error::Syntax(code, line, column) => {
                Error::JSONError(JSONError::new(code, line, column))
            }
            serde_json::error::Error::Io(err) => Error::IOError(err),
            serde_json::error::Error::FromUtf8(_) => Error::ParseError,
        }
    }
}


impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self {
            Error::AppNotExists => "the application has not been found",
            Error::ParseError => "there was an error in some parsing process",
            Error::JSONError(ref e) => e.description(),
            Error::CodeNotFound => "the code was not found in the file",
            Error::Config => "there was an error in the configuration",
            Error::IOError(ref e) => e.description(),
            Error::Unknown => "an unknown error occurred",
        }
    }

    fn cause(&self) -> Option<&StdError> {
        match *self {
            Error::IOError(ref e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct JSONError {
    code: JSONErrorCode,
    description: String,
    line: usize,
    column: usize,
}

impl JSONError {
    fn new(code: JSONErrorCode, line: usize, column: usize) -> JSONError {
        let desc = format!("{:?} at line {} column {}", code, line, column);
        JSONError {
            code: code,
            description: desc,
            line: line,
            column: column,
        }
    }
    fn description(&self) -> &str {
        self.description.as_str()
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub enum Criticity {
    Warning,
    Low,
    Medium,
    High,
    Critical,
}

impl Display for Criticity {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), fmt::Error> {
        write!(f, "{}", format!("{:?}", self).to_lowercase())
    }
}

impl Serialize for Criticity {
    fn serialize<S>(&self, serializer: &mut S) -> result::Result<(), S::Error>
        where S: Serializer
    {
        try!(serializer.serialize_str(format!("{}", self).as_str()));
        Ok(())
    }
}

impl FromStr for Criticity {
    type Err = Error;
    fn from_str(s: &str) -> Result<Criticity> {
        match s.to_lowercase().as_str() {
            "critical" => Ok(Criticity::Critical),
            "high" => Ok(Criticity::High),
            "medium" => Ok(Criticity::Medium),
            "low" => Ok(Criticity::Low),
            "warning" => Ok(Criticity::Warning),
            _ => Err(Error::ParseError),
        }
    }
}

fn print_error<S: AsRef<str>>(error: S, verbose: bool) {
    io::stderr()
        .write(&format!("{} {}\n", "Error:".bold().red(), error.as_ref().red()).into_bytes()[..])
        .unwrap();

    if !verbose {
        println!("If you need more information, try to run the program again with the {} flag.",
                 "-v".bold());
    }
}

fn print_warning<S: AsRef<str>>(warning: S, verbose: bool) {
    io::stderr()
        .write(&format!("{} {}\n",
                        "Warning:".bold().yellow(),
                        warning.as_ref().yellow())
            .into_bytes()[..])
        .unwrap();

    if !verbose {
        println!("If you need more information, try to run the program again with the {} flag.",
                 "-v".bold());
    }
}

fn print_vulnerability<S: AsRef<str>>(text: S, criticity: Criticity) {
    let text = text.as_ref();
    let start = format!("Possible {} criticity vulnerability found!:", criticity);
    let (start, message) = match criticity {
        Criticity::Low => (start.cyan(), text.cyan()),
        Criticity::Medium => (start.yellow(), text.yellow()),
        Criticity::High | Criticity::Critical => (start.red(), text.red()),
        _ => return,
    };
    println!("{} {}", start, message);
}

fn get_code(code: &str, s_line: usize, e_line: usize) -> String {
    let mut result = String::new();
    for (i, text) in code.lines().enumerate() {
        if i >= (e_line + 5) {
            break;
        } else if (s_line >= 5 && i > s_line - 5) || (s_line < 5 && i < s_line + 5) {
            result.push_str(text);
            result.push_str("\n");
        }
    }
    result
}

fn get_help_menu() -> ArgMatches<'static> {
    App::new("Android Anti-Revelation Project")
        .version(crate_version!())
        .author("Iban Eguia <razican@protonmail.ch>")
        .about("Audits Android apps for vulnerabilities")
        .arg(Arg::with_name("package")
            .help("The package string of the application to test.")
            .value_name("package")
            .required(true)
            .takes_value(true))
        .arg(Arg::with_name("verbose")
            .short("v")
            .long("verbose")
            .conflicts_with("quiet")
            .help("If you'd like the auditor to talk more than neccesary."))
        .arg(Arg::with_name("force")
            .long("force")
            .help("If you'd like to force the auditor to do everything from the beginning."))
        .arg(Arg::with_name("bench")
            .long("bench")
            .help("Show benchmarks for the analysis."))
        .arg(Arg::with_name("quiet")
            .short("q")
            .long("quiet")
            .conflicts_with("verbose")
            .help("If you'd like a zen auditor that won't talk unless it's 100% neccesary."))
        .get_matches()
}

/// Copies the contents of `from` to `to`
///
/// If the destination folder doesn't exist is created. Note that the parent folder must exist. If
/// files in the destination folder exist with the same name as in the origin folder, they will be
/// overwriten.
pub fn copy_folder<P: AsRef<Path>>(from: P, to: P) -> Result<()> {
    if !to.as_ref().exists() {
        try!(fs::create_dir(to.as_ref()));
    }

    for f in try!(fs::read_dir(from.as_ref())) {
        let f = try!(f);
        if f.path().is_dir() {
            try!(copy_folder(f.path(), to.as_ref().join(f.path().file_name().unwrap())));
        } else {
            try!(fs::copy(f.path(), to.as_ref().join(f.path().file_name().unwrap())));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use {get_code, Criticity, file_exists};
    use std::fs;
    use std::fs::File;
    use std::str::FromStr;

    #[test]
    fn it_get_code() {
        let code = "Lorem ipsum dolor sit amet, consectetur adipiscing elit.\nCurabitur tortor. \
                    Pellentesque nibh. Aenean quam.\nSed lacinia, urna non tincidunt mattis, \
                    tortor neque\nPraesent blandit dolor. Sed non quam. In vel mi\nSed aliquet \
                    risus a tortor. Integer id quam. Morbi mi.\nNullam mauris orci, aliquet et, \
                    iaculis et, viverra vitae, ligula.\nPraesent mauris. Fusce nec tellus sed \
                    ugue semper porta. Mauris massa.\nProin ut ligula vel nunc egestas porttitor. \
                    Morbi lectus risus,\nVestibulum sapien. Proin quam. Etiam ultrices. \
                    Suspendisse in\nVestibulum tincidunt malesuada tellus. Ut ultrices ultrices \
                    enim.\nAenean laoreet. Vestibulum nisi lectus, commodo ac, facilisis\nInteger \
                    nec odio. Praesent libero. Sed cursus ante dapibus diam.\nPellentesque nibh. \
                    Aenean quam. In scelerisque sem at dolor.\nSed lacinia, urna non tincidunt \
                    mattis, tortor neque adipiscing\nVestibulum ante ipsum primis in faucibus \
                    orci luctus et ultrices";

        assert_eq!(get_code(code, 1, 1),
                   "Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n\
                    Curabitur tortor. Pellentesque nibh. Aenean quam.\n\
                    Sed lacinia, urna non tincidunt mattis, tortor neque\n\
                    Praesent blandit dolor. Sed non quam. In vel mi\n\
                    Sed aliquet risus a tortor. Integer id quam. Morbi mi.\n\
                    Nullam mauris orci, aliquet et, iaculis et, viverra vitae, ligula.\n");

        assert_eq!(get_code(code, 13, 13),
                   "Vestibulum tincidunt malesuada tellus. Ut ultrices ultrices enim.\n\
                    Aenean laoreet. Vestibulum nisi lectus, commodo ac, facilisis\n\
                    Integer nec odio. Praesent libero. Sed cursus ante dapibus diam.\n\
                    Pellentesque nibh. Aenean quam. In scelerisque sem at dolor.\n\
                    Sed lacinia, urna non tincidunt mattis, tortor neque adipiscing\n\
                    Vestibulum ante ipsum primis in faucibus orci luctus et ultrices\n");

        assert_eq!(get_code(code, 7, 7),
                   "Praesent blandit dolor. Sed non quam. In vel mi\n\
                    Sed aliquet risus a tortor. Integer id quam. Morbi mi.\n\
                    Nullam mauris orci, aliquet et, iaculis et, viverra vitae, ligula.\n\
                    Praesent mauris. Fusce nec tellus sed ugue semper porta. Mauris massa.\n\
                    Proin ut ligula vel nunc egestas porttitor. Morbi lectus risus,\n\
                    Vestibulum sapien. Proin quam. Etiam ultrices. Suspendisse in\n\
                    Vestibulum tincidunt malesuada tellus. Ut ultrices ultrices enim.\n\
                    Aenean laoreet. Vestibulum nisi lectus, commodo ac, facilisis\n\
                    Integer nec odio. Praesent libero. Sed cursus ante dapibus diam.\n");

        assert_eq!(get_code(code, 7, 9),
                   "Praesent blandit dolor. Sed non quam. In vel mi\n\
                    Sed aliquet risus a tortor. Integer id quam. Morbi mi.\n\
                    Nullam mauris orci, aliquet et, iaculis et, viverra vitae, ligula.\n\
                    Praesent mauris. Fusce nec tellus sed ugue semper porta. Mauris massa.\n\
                    Proin ut ligula vel nunc egestas porttitor. Morbi lectus risus,\n\
                    Vestibulum sapien. Proin quam. Etiam ultrices. Suspendisse in\n\
                    Vestibulum tincidunt malesuada tellus. Ut ultrices ultrices enim.\n\
                    Aenean laoreet. Vestibulum nisi lectus, commodo ac, facilisis\n\
                    Integer nec odio. Praesent libero. Sed cursus ante dapibus diam.\n\
                    Pellentesque nibh. Aenean quam. In scelerisque sem at dolor.\n\
                    Sed lacinia, urna non tincidunt mattis, tortor neque adipiscing\n");
    }

    #[test]
    fn it_file_exists() {
        if file_exists("test.txt") {
            fs::remove_file("test.txt").unwrap();
        }
        assert!(!file_exists("test.txt"));
        File::create("test.txt").unwrap();
        assert!(file_exists("test.txt"));
        fs::remove_file("test.txt").unwrap();
        assert!(!file_exists("test.txt"));
    }

    #[test]
    fn it_criticity() {
        assert_eq!(Criticity::from_str("warning").unwrap(), Criticity::Warning);
        assert_eq!(Criticity::from_str("Warning").unwrap(), Criticity::Warning);
        assert_eq!(Criticity::from_str("WARNING").unwrap(), Criticity::Warning);

        assert_eq!(Criticity::from_str("low").unwrap(), Criticity::Low);
        assert_eq!(Criticity::from_str("Low").unwrap(), Criticity::Low);
        assert_eq!(Criticity::from_str("LOW").unwrap(), Criticity::Low);

        assert_eq!(Criticity::from_str("medium").unwrap(), Criticity::Medium);
        assert_eq!(Criticity::from_str("Medium").unwrap(), Criticity::Medium);
        assert_eq!(Criticity::from_str("MEDIUM").unwrap(), Criticity::Medium);

        assert_eq!(Criticity::from_str("high").unwrap(), Criticity::High);
        assert_eq!(Criticity::from_str("High").unwrap(), Criticity::High);
        assert_eq!(Criticity::from_str("HIGH").unwrap(), Criticity::High);

        assert_eq!(Criticity::from_str("critical").unwrap(),
                   Criticity::Critical);
        assert_eq!(Criticity::from_str("Critical").unwrap(),
                   Criticity::Critical);
        assert_eq!(Criticity::from_str("CRITICAL").unwrap(),
                   Criticity::Critical);

        assert!(Criticity::Warning < Criticity::Low);
        assert!(Criticity::Warning < Criticity::Medium);
        assert!(Criticity::Warning < Criticity::High);
        assert!(Criticity::Warning < Criticity::Critical);
        assert!(Criticity::Low < Criticity::Medium);
        assert!(Criticity::Low < Criticity::High);
        assert!(Criticity::Low < Criticity::Critical);
        assert!(Criticity::Medium < Criticity::High);
        assert!(Criticity::Medium < Criticity::Critical);
        assert!(Criticity::High < Criticity::Critical);

        assert_eq!(format!("{}", Criticity::Warning).as_str(), "warning");
        assert_eq!(format!("{}", Criticity::Low).as_str(), "low");
        assert_eq!(format!("{}", Criticity::Medium).as_str(), "medium");
        assert_eq!(format!("{}", Criticity::High).as_str(), "high");
        assert_eq!(format!("{}", Criticity::Critical).as_str(), "critical");

        assert_eq!(format!("{:?}", Criticity::Warning).as_str(), "Warning");
        assert_eq!(format!("{:?}", Criticity::Low).as_str(), "Low");
        assert_eq!(format!("{:?}", Criticity::Medium).as_str(), "Medium");
        assert_eq!(format!("{:?}", Criticity::High).as_str(), "High");
        assert_eq!(format!("{:?}", Criticity::Critical).as_str(), "Critical");
    }
}
