use std::process::ExitCode;

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

fn try_main() -> Result<(), Box<dyn std::error::Error>> {
	let arg = std::env::args().nth(1);
	let filename = arg.unwrap_or("prospero.vm".into());
	let text = std::fs::read_to_string(&filename)
		.map_err(|e| format!("couldn't read prospero.vm: {e}"))?;
	let mut ops = vec![];
	let mut index = 0;
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
		_ = index;
		debug_assert!(tag.starts_with('_') && u16::from_str_radix(&tag[1..], 16).unwrap() == index);
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
	const BIT_OFFSET: u32 = 2 + 4 * 3 + 4 + 2 * 4 + 2 * 3;
	let width: u16 = 512;
	let height: u16 = 512;
	if width % 8 != 0 {
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
									Op::Const(v) => v,
									Op::Add(x, y) => buf.get(x) + buf.get(y),
									Op::Sub(x, y) => buf.get(x) - buf.get(y),
									Op::Mul(x, y) => buf.get(x) * buf.get(y),
									Op::Min(x, y) => buf.get(x).min(buf.get(y)),
									Op::Max(x, y) => buf.get(x).max(buf.get(y)),
									Op::VarX => (x8 * 8 + bit) as f32 * inv_width2 - 1.0,
									Op::VarY => y as f32 * inv_height2 - 1.0,
									Op::Neg(x) => -buf.get(x),
									Op::Sqrt(x) => buf.get(x).sqrt(),
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
