use std::io::Write;

#[allow(unused)]//TODO
#[derive(Copy, Clone, Debug)]
enum Op {
	Const(f32),
	VarX,
	VarY,
	Mul(u16, u16),
	Add(u16, u16),
	Sub(u16, u16),
	Neg(u16),
	Sqrt(u16),
	Max(u16, u16),
	Min(u16, u16),
}

fn parse_var(var: &str) -> u16 {
	assert!(var.starts_with('_'));
	u16::from_str_radix(&var[1..], 16).unwrap()
}

fn main() {
	let text = std::fs::read_to_string("prospero.vm").expect("couldn't read prospero.vm");
	let mut ops = vec![];
	for line in text.split('\n') {
		let line = line.trim_ascii();
		if line.starts_with('#') { continue; }
		if line.is_empty() { continue; }
		let mut words = line.split(' ');
		words.next().unwrap();
		let op = words.next().unwrap();
		ops.push(match op {
			"const" => {
				let arg: f32 = words.next().unwrap().parse().unwrap();
				Op::Const(arg)
			}
			"var-x" => Op::VarX,
			"var-y" => Op::VarY,
			"add" => {
				let arg1 = words.next().unwrap();
				let arg2 = words.next().unwrap();
				Op::Add(parse_var(arg1), parse_var(arg2))
			}
			"sub" => {
				let arg1 = words.next().unwrap();
				let arg2 = words.next().unwrap();
				Op::Sub(parse_var(arg1), parse_var(arg2))
			}
			"mul" => {
				let arg1 = words.next().unwrap();
				let arg2 = words.next().unwrap();
				Op::Mul(parse_var(arg1), parse_var(arg2))
			}
			"min" => {
				let arg1 = words.next().unwrap();
				let arg2 = words.next().unwrap();
				Op::Min(parse_var(arg1), parse_var(arg2))
			}
			"max" => {
				let arg1 = words.next().unwrap();
				let arg2 = words.next().unwrap();
				Op::Max(parse_var(arg1), parse_var(arg2))
			}
			"neg" => {
				let arg = words.next().unwrap();
				Op::Neg(parse_var(arg))
			}
			"sqrt" => {
				let arg = words.next().unwrap();
				Op::Sqrt(parse_var(arg))
			}
			"square" => {
				let arg = parse_var(words.next().unwrap());
				Op::Mul(arg, arg)
			}
			_ => {
				panic!("bad format: {op}");
			}
		});
	}
	let mut out_image =
		std::io::BufWriter::new(std::fs::File::create("out.bmp").unwrap());
	out_image.write_all(b"BM").unwrap();
	let bit_offset = 2 + 4 * 3 + 4 + 2 * 4 + 2*3;
	let width = 1024;
	let height = 1024;
	let file_size = bit_offset + u32::from(width) * u32::from(height) * 8;
	out_image.write_all(&u32::to_le_bytes(file_size)).unwrap();
	out_image.write_all(&[0,0,0,0]).unwrap();
	out_image.write_all(&u32::to_le_bytes(bit_offset)).unwrap();
	let header_size = 12;
	out_image.write_all(&u32::to_le_bytes(header_size)).unwrap();
	out_image.write_all(&u16::to_le_bytes(width)).unwrap();
	out_image.write_all(&u16::to_le_bytes(height)).unwrap();
	out_image.write_all(&u16::to_le_bytes(1)).unwrap(); // planes
	out_image.write_all(&u16::to_le_bytes(1)).unwrap(); // bpp
	out_image.write_all(&[0, 0, 0, 255, 255, 255]).unwrap(); // colors
	for y in 0..height {
		for x in 0..width/8 {
			out_image.write_all(&[(x as u8).wrapping_add(y as u8)]).unwrap();
		}
	}
	
}
