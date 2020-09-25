#[macro_use]
extern crate log;
extern crate ansi_term;
extern crate getopts;
extern crate glob;
extern crate regex;
extern crate simple_logger;
extern crate tempfile;

mod drt;

use ansi_term::Colour::{Green, Red, Yellow};
use drt::cmd::exectable_full_path;
use drt::cmd::cmdline;
use drt::DestFile;
use drt::diff::diff;
use drt::diff::DiffStatus;
use drt::err::DrtError;
use drt::err::log_cmd_action;
use drt::err::Verb;
use drt::GenFile;
use drt::Mode;
use drt::SrcFile;
use drt::template::{update_from_template, generate_recommended_file, replace_line, ChangeString};
use drt::userinput::ask;
use getopts::Options;
use log::LevelFilter;
use simple_logger::SimpleLogger;
use std::collections::HashMap;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::slice::Iter;
use std::str;
use log::trace;

fn create_or_diff(
    mode: Mode,
    template: &SrcFile,
    dest: &DestFile,
    gen: &GenFile,
) -> Result<DiffStatus, DrtError> {
    debug!("create_or_diff: diff");
    diff(gen.path(), dest.path());
    match update_from_template(mode, template, gen, dest) {
        Ok(_) => {
            Ok(diff(gen.path(), dest.path()))
        },
        Err(e) => Err(e)
    }
}
fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} [options]", program);
    println!("{}", opts.usage(&brief));
    println!("COMMANDS");
    println!();
    println!("v key value            set template variable ");
    println!("t infile outfile       copy infile to outfile replacing @@key@@ with value  ");
    println!("x command arg1 arg2    run command  ");
    println!("-- x command -arg      run command (add -- to make sure hyphens are passed on");
}
#[derive(Debug)]
enum Action {
    Template(String, String),
    Execute(String),
    Error,
    None,
}
#[derive(Debug)]
enum Type {
    Template,
    Execute,
    //InputFile,
    //OutputFile,
    Variable,
    Unknown,
}
#[test]
fn test_parse_type() {
    match parse_type(&String::from("t")) {
        Type::Template => {}
        _ => panic!("expected Template"),
    }
    match parse_type(&String::from("x")) {
        Type::Execute => {}
        _ => panic!("expected Execute"),
    }
    match parse_type(&String::from("v")) {
        Type::Variable => {}
        _ => panic!("expected Template"),
    }
}
fn parse_type(input: &str) -> Type {
    match input {
        "t" => Type::Template,
        "x" => Type::Execute,
        "v" => Type::Variable,
        _ => {
            debug!("Unknown {}", input);
            Type::Unknown
        }
    }
}
fn process_template_file<'t>(
    mode: Mode,
    vars: &'t HashMap<&'_ str, &'_ str>,
    template: &SrcFile,
    dest: &DestFile,
) -> Result<DiffStatus, DrtError> {
    let gen = generate_recommended_file(vars, template)?;
    create_or_diff(mode, template, dest, &gen)
}
#[test]
fn test_execute_active() -> Result<(), DrtError> {
    execute_active("/bin/true")?;
    match execute_active("/bin/false") {
        Err(e) => println!( "{} {}", Red.paint("Not Executable: "), Red.paint(e.to_string())),
        _ => return Err(DrtError::Error)
    }
    execute_active("echo echo_ping")?;
    Ok(())
}

fn execute_inactive(raw_cmd: &str) -> Result<(), DrtError> {
	let empty_vec: Vec<&str> = Vec::new();
	let v: Vec<&str> = raw_cmd.split(' ').collect();
	let (cmd,args) : (&str, Vec<&str>) = match v.as_slice() {
		[] => ("", empty_vec),
		//[cmd] => (cmd, empty_vec),
		[cmd, args @ ..] => (cmd, args.to_vec()),
	};
	match cmd {
		"" => Err(DrtError::ExpectedArg("x command")),
		_ => {
		trace!("{}", cmd);
		let exe_path = exectable_full_path(cmd)?;
		trace!("{:?}", exe_path);
		trace!("{:?}", args);
		let cli = cmdline(exe_path.display().to_string(), args);
		log_cmd_action("run", Verb::WOULD, cli);
		Ok(())
		}
	}
}
fn execute_active(cmd: &str) -> Result<(), DrtError> {
    let parts: Vec<&str> = cmd.split(' ').collect();
    let output = Command::new(parts[0])
        .args(&parts[1..])
        .output()
        .expect("cmd failed");
    println!("{} {}", Green.paint("LIVE: run "), Green.paint(cmd));
    io::stdout()
        .write_all(&output.stdout)
        .expect("error writing to stdout");
    match output.status.code() {
        Some(n) => {
            if n == 0 {
                println!(
                    "{} {}",
                    Green.paint("status code: "),
                    Green.paint(n.to_string())
                );
                Ok(())
            } else {
                Err(DrtError::NotZeroExit(n))
            }
        }
        None => {
            Err(DrtError::CmdExitedPrematurely)
        }
    }
}
fn execute_interactive(cmd: &str) -> Result<(), DrtError> {
    match ask(&format!("run (y/n): {}", cmd)) {
        'n' => {
            println!("{} {}", Yellow.paint("WOULD: run "), Yellow.paint(cmd));
            Ok(())
        }
        'y' => execute_active(cmd),
        _ => execute_interactive(cmd),
    }
}
fn execute(mode: Mode, cmd: &str) -> Result<(), DrtError> {
    match mode {
        Mode::Interactive => execute_interactive(cmd),
        Mode::Passive => execute_inactive(cmd).map(|_pathbuf|()),
        Mode::Active => execute_active(cmd),
    }
}

fn do_action<'g>(
    mode: Mode,
    vars: &'g HashMap<&'g str, &'g str>,
    action: Action,
) -> Result<(), DrtError> {
    match action {
        Action::Template(template_file_name, output_file_name) => {
            let template_file = SrcFile::new(PathBuf::from(template_file_name));
            let output_file = DestFile::new(mode, PathBuf::from(output_file_name));

            match process_template_file(mode, &vars, &template_file, &output_file) {
                Err(e) => {
                    println!("do_action: {} {}", Red.paint("error:"), Red.paint(e.to_string()));
                    Err(e)
                }
                _ => Ok(())
            }
        },
        Action::Execute(cmd) => {
            let the_cmd = match replace_line(vars, cmd.clone())? {
                ChangeString::Changed(new_cmd) => new_cmd,
                ChangeString::Unchanged => cmd,
            };
            match execute(mode, &the_cmd) {
                Ok(()) => Ok(()),
                Err(e) => {
                    println!("do_action: {} {}", Red.paint("error:"), Red.paint(e.to_string()));
                    Err(e)
                }
            }
        },
        Action::Error => { Err(DrtError::Error)},
        Action::None => Ok(()),
    }
}

#[test]
fn test_do_action() {
    let mut vars: HashMap<&str, &str> = HashMap::new();
    vars.insert("value", "unit_test");
    let template = Action::Template(
        String::from("template/test.config"),
        String::from("template/out.config"),
    );
    match do_action(Mode::Passive, &vars, template) {
        Ok(_) => {}
        Err(_) => std::process::exit(1),
    }
}
fn expect_option<R>(a: Option<R>, emsg: &str) -> Result<R, DrtError> {
    match a {
        Some(r) => Ok(r),
        None => {
            println!(
                "{}",
                Red.paint(emsg)
            );
            Err(DrtError::Warn)
        }
    }
    
}
fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optflag("D", "debug", "debug logging");
    opts.optflag("i", "interactive", "ask before overwrite");
    opts.optflag("a", "active", "overwrite without asking");
    opts.optflag("h", "help", "print this help menu");
    let matches = opts.parse(&args[1..]).unwrap();

    if matches.opt_present("h") {
        print_usage(&program, opts);
        return;
    }
    if matches.opt_present("debug") {
        SimpleLogger::new()
            .with_level(LevelFilter::Trace)
            .init()
            .expect("log inti error");
    } else {
        SimpleLogger::new()
            .with_level(LevelFilter::Warn)
            .init()
            .expect("log inti error");
    }
    let drt_active_env = env::var("DRT_ACTIVE").is_ok();
    if drt_active_env {
        debug!(
            "DRT_ACTIVE enabled DRT_ACTIVE= {:#?}",
            env::var("DRT_ACTIVE").unwrap()
        );
    } else {
        debug!("DRT_ACTIVE not set");
    }
    let mode = if matches.opt_present("interactive") {
        Mode::Interactive
    } else if matches.opt_present("active") | drt_active_env {
        Mode::Active
    } else {
        Mode::Passive
    };
    let mut vars: HashMap<&str, &str> = HashMap::new();
    {
        let mut input_list: Iter<String> = matches.free.iter();
        while let Some(input) = input_list.next() {
            let t: Type = parse_type(input);
            let mut cmd = String::new();
            let action = match t {
                Type::Template => {
                    let infile = String::from(
                        input_list
                            .next()
                            .expect("expected template: tp template output"),
                    );
                    let outfile = String::from(
                        input_list
                            .next()
                            .expect("expected output: tp template output"),
                    );
                    Action::Template(infile, outfile)
                }
                Type::Variable => {
                    match expect_option(input_list.next(), "expected key: v key value") {
                        Ok(k) => {
                            match expect_option(input_list.next(), "expected value: v key value") {
                                Ok(v) => {
                                    vars.insert(k, v);
                                    Action::None
                                },
                                Err(_) => Action::Error
                            }
                        },
                        Err(e) => {
                            println!("Variable: {} {}", Red.paint("error:"), Red.paint(e.to_string()));
                            Action::Error
                        }
                    }
                }
                Type::Execute => {
                    #[allow(clippy::while_let_on_iterator)]
                    while let Some(input) = input_list.next() {
                        if cmd.is_empty() {
                            cmd.push_str(&input.to_string());
                        } else {
                            cmd.push_str(" ");
                            cmd.push_str(&input.to_string());
                        }
                    }
                    //let cmd_str: &str = cmd.as_str();
                    Action::Execute(cmd)
                }
                Type::Unknown => {
                    println!("{} {}", Red.paint("Unknown type:"), Red.paint(input));
                    Action::Error
                }
            };
            //debug!("vars {:#?}", &vars);
            debug!("action {:#?}", action);
            match do_action(mode, &vars, action) {
                Ok(a) => a,
                Err(_) => std::process::exit(1),
            }
        }
    }
}


