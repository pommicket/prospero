use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Copy, Clone, Debug)]
enum Op {
	VarX,
	VarY,
	Mul(u16, u16),
	Add(u16, u16),
	Sub(u16, u16),
	Sqrt(u16),
	Max(u16, u16),
	Min(u16, u16),
	MulConst(u16, f32),
	AddConst(u16, f32),
	SubConst(u16, f32),
	MaxConst(u16, f32),
	MinConst(u16, f32),
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

fn gcd16(mut x: u16, mut y: u16) -> u16 {
	while x != 0 {
		(x, y) = (y % x, x);
	}
	y
}

fn available_parallelism() -> Option<u16> {
	std::thread::available_parallelism()
		.ok()?
		.get()
		.try_into()
		.ok()
}

#[derive(Copy, Clone)]
struct PixelBuffer(*mut u8);
impl PixelBuffer {
	unsafe fn write(&mut self, val: u8) {
		unsafe {
			*self.0 = val;
			self.0 = self.0.add(1);
		}
	}
	#[must_use]
	unsafe fn offset(self, by: usize) -> Self {
		Self(unsafe { self.0.add(by) })
	}
}
unsafe impl Send for PixelBuffer {}
unsafe impl Sync for PixelBuffer {}

enum Value {
	Var(Op),
	Constant(f32),
}

#[derive(Clone, Copy)]
enum ValueId {
	Var(u16),
	Constant(f32),
}

impl Value {
	fn add(a: ValueId, b: ValueId) -> Value {
		match (a, b) {
			(ValueId::Constant(a), ValueId::Constant(b)) => Value::Constant(a + b),
			(ValueId::Var(a), ValueId::Var(b)) => Value::Var(Op::Add(a, b)),
			(ValueId::Var(a), ValueId::Constant(b)) => Value::Var(Op::AddConst(a, b)),
			(ValueId::Constant(a), ValueId::Var(b)) => Value::Var(Op::AddConst(b, a)),
		}
	}
	fn sub(a: ValueId, b: ValueId) -> Value {
		match (a, b) {
			(ValueId::Constant(a), ValueId::Constant(b)) => Value::Constant(a - b),
			(ValueId::Var(a), ValueId::Var(b)) => Value::Var(Op::Sub(a, b)),
			(ValueId::Var(a), ValueId::Constant(b)) => Value::Var(Op::AddConst(a, -b)),
			(ValueId::Constant(a), ValueId::Var(b)) => Value::Var(Op::SubConst(b, a)),
		}
	}
	fn mul(a: ValueId, b: ValueId) -> Value {
		match (a, b) {
			(ValueId::Constant(a), ValueId::Constant(b)) => Value::Constant(a * b),
			(ValueId::Var(a), ValueId::Var(b)) => Value::Var(Op::Mul(a, b)),
			(ValueId::Var(a), ValueId::Constant(b)) => Value::Var(Op::MulConst(a, b)),
			(ValueId::Constant(a), ValueId::Var(b)) => Value::Var(Op::MulConst(b, a)),
		}
	}
	fn min(a: ValueId, b: ValueId) -> Value {
		match (a, b) {
			(ValueId::Constant(a), ValueId::Constant(b)) => Value::Constant(a.min(b)),
			(ValueId::Var(a), ValueId::Var(b)) => Value::Var(Op::Min(a, b)),
			(ValueId::Var(a), ValueId::Constant(b)) => Value::Var(Op::MinConst(a, b)),
			(ValueId::Constant(a), ValueId::Var(b)) => Value::Var(Op::MinConst(b, a)),
		}
	}
	fn max(a: ValueId, b: ValueId) -> Value {
		match (a, b) {
			(ValueId::Constant(a), ValueId::Constant(b)) => Value::Constant(a.max(b)),
			(ValueId::Var(a), ValueId::Var(b)) => Value::Var(Op::Max(a, b)),
			(ValueId::Var(a), ValueId::Constant(b)) => Value::Var(Op::MaxConst(a, b)),
			(ValueId::Constant(a), ValueId::Var(b)) => Value::Var(Op::MaxConst(b, a)),
		}
	}
	fn sqrt(a: ValueId) -> Value {
		match a {
			ValueId::Constant(c) => Value::Constant(c.sqrt()),
			ValueId::Var(o) => Value::Var(Op::Sqrt(o)),
		}
	}
	fn neg(a: ValueId) -> Value {
		Value::sub(ValueId::Constant(0.0), a)
	}
}

fn try_main() -> Result<(), Box<dyn std::error::Error>> {
	let arg = std::env::args().nth(1);
	let filename = arg.unwrap_or("prospero.vm".into());
	let text = std::fs::read_to_string(&filename)
		.map_err(|e| format!("couldn't read prospero.vm: {e}"))?;
	let mut ops = vec![];
	let mut index = 0;
	let mut mapping = HashMap::new();
	for line in text.split('\n') {
		let line = line.trim_ascii();
		if line.starts_with('#') {
			continue;
		}
		if line.is_empty() {
			continue;
		}
		let mut words = line.split(' ');
		let tag = words.next().unwrap();
		debug_assert!(tag.starts_with('_') && u16::from_str_radix(&tag[1..], 16).unwrap() == index);

		let op = words.next().unwrap();
		let value = match op {
			"const" => {
				let arg: f32 = words.next().unwrap().parse().unwrap();
				Value::Constant(arg)
			}
			"var-x" => Value::Var(Op::VarX),
			"var-y" => Value::Var(Op::VarY),
			"add" => {
				let arg1 = mapping[&parse_var(words.next().unwrap())];
				let arg2 = mapping[&parse_var(words.next().unwrap())];
				Value::add(arg1, arg2)
			}
			"sub" => {
				let arg1 = mapping[&parse_var(words.next().unwrap())];
				let arg2 = mapping[&parse_var(words.next().unwrap())];
				Value::sub(arg1, arg2)
			}
			"mul" => {
				let arg1 = mapping[&parse_var(words.next().unwrap())];
				let arg2 = mapping[&parse_var(words.next().unwrap())];
				Value::mul(arg1, arg2)
			}
			"min" => {
				let arg1 = mapping[&parse_var(words.next().unwrap())];
				let arg2 = mapping[&parse_var(words.next().unwrap())];
				Value::min(arg1, arg2)
			}
			"max" => {
				let arg1 = mapping[&parse_var(words.next().unwrap())];
				let arg2 = mapping[&parse_var(words.next().unwrap())];
				Value::max(arg1, arg2)
			}
			"neg" => {
				let arg = mapping[&parse_var(words.next().unwrap())];
				Value::neg(arg)
			}
			"sqrt" => {
				let arg = mapping[&parse_var(words.next().unwrap())];
				Value::sqrt(arg)
			}
			"square" => {
				let arg = mapping[&parse_var(words.next().unwrap())];
				Value::mul(arg, arg)
			}
			_ => {
				panic!("bad format: {op}");
			}
		};
		match value {
			Value::Constant(c) => {
				mapping.insert(index, ValueId::Constant(c));
			}
			Value::Var(op) => {
				mapping.insert(index, ValueId::Var(ops.len() as _));
				ops.push(op);
			}
		}
		index = index.checked_add(1).ok_or("too many lines")?;
	}
	const BIT_OFFSET: u32 = 2 + 4 * 3 + 4 + 2 * 4 + 2 * 3;
	let width: u16 = 512;
	let height: u16 = 512;
	if !width.is_multiple_of(8) {
		return Err(format!("width {width} should be a multiple of 8").into());
	}
	let file_size = BIT_OFFSET + u32::from(width) * u32::from(height) / 8;
	let fd = unsafe {
		libc::open(
			c"out.bmp".as_ptr(),
			libc::O_RDWR | libc::O_TRUNC | libc::O_CREAT,
			0o644,
		)
	};
	if fd == -1 {
		return Err("failed to create file out.bmp".into());
	}
	if unsafe { libc::ftruncate(fd, i64::from(file_size)) } != 0 {
		return Err("failed to truncate file to length".into());
	}

	let data = unsafe {
		libc::mmap(
			std::ptr::null_mut(),
			file_size as usize,
			libc::PROT_READ | libc::PROT_WRITE,
			libc::MAP_SHARED,
			fd,
			0,
		)
	};
	if data == libc::MAP_FAILED {
		return Err("failed to mmap".into());
	}
	if unsafe { libc::close(fd) } != 0 {
		return Err("failed to close file".into());
	}
	let mut header = [0_u8; BIT_OFFSET as usize];
	header[..2].copy_from_slice(b"BM");
	header[2..6].copy_from_slice(&u32::to_le_bytes(file_size));
	// 6..10 reserved
	header[10..14].copy_from_slice(&u32::to_le_bytes(BIT_OFFSET));
	let header_size = 12;
	header[14..18].copy_from_slice(&u32::to_le_bytes(header_size));
	header[18..20].copy_from_slice(&u16::to_le_bytes(width));
	header[20..22].copy_from_slice(&u16::to_le_bytes(height));
	header[22..24].copy_from_slice(&u16::to_le_bytes(1)); // planes
	header[24..26].copy_from_slice(&u16::to_le_bytes(1)); // bpp
	header[26..BIT_OFFSET as usize].copy_from_slice(&[0, 0, 0, 255, 255, 255]); // colors
	unsafe {
		data.copy_from_nonoverlapping(header.as_ptr().cast(), header.len());
	}

	let inv_width2 = 2.0 / f32::from(width);
	let inv_height2 = 2.0 / f32::from(height);
	let thread_count = gcd16(height, available_parallelism().unwrap_or(16));
	let pixels = PixelBuffer(unsafe { data.add(BIT_OFFSET as usize).cast() });
	std::thread::scope(|s| {
		for t in 0..thread_count {
			let ops = &ops;
			s.spawn(move || {
				let mut buf: Box<Buf> = Box::default();
				let rows_per_thread = height / thread_count;
				let base_y = rows_per_thread * t;
				let mut pixel =
					unsafe { pixels.offset(usize::from(base_y) * usize::from(width) / 8) };
				for y in base_y..base_y + rows_per_thread {
					for x8 in 0..width / 8 {
						let mut byte = 0;
						for bit in 0..8 {
							for (i, op) in ops.iter().copied().enumerate() {
								let val = match op {
									Op::Add(x, y) => buf.get(x) + buf.get(y),
									Op::Sub(x, y) => buf.get(x) - buf.get(y),
									Op::Mul(x, y) => buf.get(x) * buf.get(y),
									Op::Min(x, y) => buf.get(x).min(buf.get(y)),
									Op::Max(x, y) => buf.get(x).max(buf.get(y)),
									Op::VarX => (x8 * 8 + bit) as f32 * inv_width2 - 1.0,
									Op::VarY => y as f32 * inv_height2 - 1.0,
									Op::Sqrt(x) => buf.get(x).sqrt(),
									Op::AddConst(x, y) => buf.get(x) + y,
									Op::SubConst(x, y) => y - buf.get(x),
									Op::MulConst(x, y) => buf.get(x) * y,
									Op::MinConst(x, y) => buf.get(x).min(y),
									Op::MaxConst(x, y) => buf.get(x).max(y),
								};
								buf.set(i, val);
							}
							byte |= u8::from(buf.get((ops.len() - 1) as u16) < 0.0) << (7 - bit);
						}
						unsafe {
							pixel.write(byte);
						}
					}
				}
			});
		}
	});
	_ = unsafe { libc::munmap(pixels.0.cast(), file_size as usize) };
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
