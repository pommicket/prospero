use std::process::ExitCode;
use std::process::Command;

fn try_main() -> Result<(), Box<dyn std::error::Error>> {
	let output_dir = std::env::var("OUT_DIR")?;
	let mut children = vec![];
	println!("cargo::rerun-if-changed=src/asm");
	for file in std::fs::read_dir("src/asm").map_err(|e| format!("can't read src/asm: {e}"))? {
		let file = file?;
		let file_name: String = file.file_name().into_string().expect("bad UTF-8 in file name");
		let Some(name) = file_name.strip_suffix(".asm") else {
			continue;
		};
		println!("cargo::rerun-if-changed=src/asm/{file_name}");
		children.push((name.to_owned(), Command::new("nasm")
			.args([format!("src/asm/{file_name}"), "-o".into(), format!("{output_dir}/{name}.out")])
			.spawn()
			.map_err(|e| format!("error running nasm: {e}"))?));
	}
	for (name, mut child) in children {
		let status = child.wait()?;
		if !status.success() {
			return Err(format!("Assembling {name} failed").into());
		}
	}
	Ok(())
}

fn main() -> ExitCode {
	if let Err(e) = try_main() {
		eprintln!("error: {e}");
		ExitCode::FAILURE
	} else {
		ExitCode::SUCCESS
	}
}
