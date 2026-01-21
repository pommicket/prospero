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
	debug_assert!(var.starts_with('_'));
	u16::from_str_radix(&var[1..], 16).unwrap()
}

struct Buf([f32; 65536]);
impl Buf {
	fn get(&self, i: u16) -> f32 {
		self.0[usize::from(i)]
	}
	fn set(&mut self, i: usize, value: f32) {
		self.0[i] = value;
	}
}
impl Default for Buf {
	fn default() -> Self {
		Self([0.0; 65536])
	}
}

fn main() {
	let arg = std::env::args().nth(1);
	let filename = arg.unwrap_or("prospero.vm".into());
	let text = std::fs::read_to_string(&filename).expect("couldn't read prospero.vm");
	let mut ops = vec![];
	let mut index = 0;
	for line in text.split('\n') {
		let line = line.trim_ascii();
		if line.starts_with('#') { continue; }
		if line.is_empty() { continue; }
		let mut words = line.split(' ');
		let tag = words.next().unwrap();
		_ = index;
		debug_assert!(tag.starts_with('_') && u16::from_str_radix(&tag[1..],16).unwrap()==index);
		index += 1;
		
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
	let width = 256;
	let height = 256;
	let file_size = bit_offset + u32::from(width) * u32::from(height) / 8;
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
	let mut buf: Box<Buf> = Box::default();
	let inv_width = 1.0 / f32::from(width);
	let inv_height = 1.0 / f32::from(height);
	for y in 0..height {
		for x8 in 0..width/8 {
			let mut byte = 0;
			for bit in 0..8 {
				for (i, op) in ops.iter().copied().enumerate() {
					let val = match op {
						Op::Const(v) => v,
						Op::Add(x, y) => {
							buf.get(x) + buf.get(y)
						}
						Op::Sub(x, y) => {
							buf.get(x) - buf.get(y)
						}
						Op::Mul(x, y) => {
							buf.get(x) * buf.get(y)
						}
						Op::Min(x, y) => {
							buf.get(x).min(buf.get(y))
						}
						Op::Max(x, y) => {
							buf.get(x).max(buf.get(y))
						}
						Op::VarX => {
							(x8 * 8 + bit) as f32 * inv_width * 2.0 - 1.0
						}
						Op::VarY => {
							y as f32 * inv_height * 2.0 - 1.0
						}
						Op::Neg(x) => -buf.get(x),
						Op::Sqrt(x) => buf.get(x).sqrt()
					};
					buf.set(i, val);
				}
				byte |= u8::from(buf.get((ops.len() - 1) as u16) < 0.0) << (7-bit);
			}
			out_image.write_all(&[byte]).unwrap();
		}
	}
	
}
