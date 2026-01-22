#![allow(dead_code)] //TODO
use std::collections::HashMap;
use std::error::Error;
use std::process::ExitCode;

const SINGLE_THREADED: bool = true;

macro_rules! include_asm {
	($name:literal) => {
		include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".out"))
	};
}

#[allow(dead_code)] // TODO
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

fn available_parallelism() -> Option<u16> {
	if SINGLE_THREADED {
		return Some(1);
	}
	let result: u16 = std::thread::available_parallelism()
		.ok()?
		.get()
		.try_into()
		.ok()?;
	Some(result.min(16384))
}

#[derive(Copy, Clone)]
struct PixelBuffer(*mut u8);
impl PixelBuffer {
	#[must_use]
	unsafe fn offset(self, by: usize) -> Self {
		Self(unsafe { self.0.add(by) })
	}
	unsafe fn ptr(self) -> *mut u8 {
		self.0
	}
}
unsafe impl Send for PixelBuffer {}
unsafe impl Sync for PixelBuffer {}

#[derive(Copy, Clone)]
struct Code(*mut u8);
unsafe impl Send for Code {}
unsafe impl Sync for Code {}
impl Code {
	#[allow(clippy::too_many_arguments)]
	unsafe fn run(
		self,
		output_data: *mut u8,
		count: u16,
		x_strides: &ZmmValue,
		stride16: f32,
		y_pos: f32,
		buffer: &mut [ZmmValue],
		constants: &[ZmmValue],
	) {
		let function: unsafe extern "sysv64" fn(
			*mut u8,
			u64,
			*const f32,
			f32,
			f32,
			*mut ZmmValue,
			*const ZmmValue,
		) = unsafe { std::mem::transmute(self.0) };
		let count: u64 = count.into();
		let x_strides: *const f32 = x_strides.as_ptr();
		let buffer = buffer.as_mut_ptr();
		let constants = constants.as_ptr();
		unsafe {
			function(
				output_data,
				count,
				x_strides,
				stride16,
				y_pos,
				buffer,
				constants,
			)
		};
	}
}

unsafe fn wrap_code(core: &[u8]) -> Result<Code, Box<dyn Error>> {
	let base_template = include_asm!("base");
	let marker_idx = base_template
		.windows(16)
		.position(|win| win == [0xcc; 16])
		.expect("bad ASM");
	let base_template_prefix = &base_template[..marker_idx];
	let base_template_suffix = &base_template[marker_idx + 16..];
	let map_size = (base_template.len() + core.len() + 128).next_multiple_of(4096);
	if map_size > (1 << 30) {
		return Err("map too large. i'm scared.".into());
	}
	let code = unsafe {
		libc::mmap(
			std::ptr::null_mut(),
			map_size,
			libc::PROT_READ | libc::PROT_WRITE,
			libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
			-1,
			0,
		)
	};
	if code == libc::MAP_FAILED {
		return Err("mmap failed".into());
	}
	let code: *mut u8 = code.cast();
	unsafe {
		code.copy_from_nonoverlapping(base_template_prefix.as_ptr(), base_template_prefix.len());
	}
	let ptr = unsafe { code.add(base_template_prefix.len()) };
	unsafe {
		ptr.copy_from_nonoverlapping(core.as_ptr(), core.len());
	}
	let ptr = unsafe { ptr.add(core.len()) };
	unsafe {
		ptr.copy_from_nonoverlapping(base_template_suffix.as_ptr(), base_template_suffix.len());
	}
	let ptr = unsafe { ptr.add(base_template_suffix.len()) };
	let jump_offset = -((core.len() + base_template_suffix.len() + 6) as i32);
	let [j0, j1, j2, j3] = jump_offset.to_le_bytes();
	let epilogue = [0x0f, 0x8f, j0, j1, j2, j3, 0xc3];
	unsafe { ptr.copy_from_nonoverlapping(epilogue.as_ptr(), epilogue.len()) };
	if unsafe { libc::mprotect(code.cast(), map_size, libc::PROT_EXEC) } != 0 {
		return Err("mprotect failed".into());
	}
	Ok(Code(code))
}

#[derive(Copy, Clone, Default)]
#[repr(C, align(64))]
struct ZmmValue([f32; 16]);
impl ZmmValue {
	fn as_ptr(&self) -> *const f32 {
		self.0.as_ptr()
	}
}

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

struct Info {
	code: Code,
	pixels: PixelBuffer,
	rows_per_thread: u16,
	x_strides: ZmmValue,
	width: u16,
	height: u16,
	constants: Vec<ZmmValue>,
}

impl Info {
	unsafe fn thread_main(&self, thread_idx: u16) {
		let base_y = self.rows_per_thread * thread_idx;
		let inv_width2 = 2.0 / f32::from(self.width);
		let inv_height2 = 2.0 / f32::from(self.height);
		let pixels = self.pixels;
		let width = self.width;
		let code = self.code;
		let x_strides = &self.x_strides;
		// TODO: pick correct size for buffer
		let mut buffer = vec![ZmmValue::default(); 10_000];
		let constants = &self.constants;
		for y in base_y..base_y + self.rows_per_thread {
			let pixel = unsafe { pixels.offset(usize::from(y) * usize::from(width) / 8) };
			unsafe {
				code.run(
					pixel.ptr(),
					width / 16,
					x_strides,
					16.0 * inv_width2,
					y as f32 * inv_height2 - 1.0,
					&mut buffer,
					constants,
				)
			};
		}
	}
}

fn read_ops(text: String) -> Result<Vec<Op>, Box<dyn Error>> {
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
	Ok(ops)
}

#[derive(Clone, Copy, Debug)]
enum Location {
	Zmm(u8),
	Buffer(u32),
	Constant(u32),
}

#[derive(Clone, Copy, Debug)]
enum LowLevelOp {
	LoadBuffer(u8, u32),
	LoadConstant(u8, u32),
	StoreBuffer(u32, u8),
	Add(u8, u8, Location),
	Sub(u8, u8, Location),
	Mul(u8, u8, Location),
	Sqrt(u8, Location),
	Min(u8, u8, Location),
	Max(u8, u8, Location),
}

fn zmm_dest_bits(r: u8) -> (u8, u8) {
	let bit4 = r >> 4;
	let bit3 = (r >> 3) & 1;
	let bits012 = r & 7;
	((bit3 << 7) | (bit4 << 4), bits012 << 3)
}

fn zmm_src1_bits(r: u8) -> (u8, u8) {
	let bit4 = r >> 4;
	let bit0123 = r & 15;
	(bit0123 << 3, bit4 << 3)
}

fn zmm_src2_bits(r: u8) -> (u8, u8) {
	let bit34 = r >> 3;
	let bit012 = r & 7;
	(bit34 << 5, bit012)
}

trait ByteWriter {
	fn write(&mut self, bytes: &[u8]);
	fn write_u32(&mut self, value: u32) {
		self.write(&value.to_le_bytes());
	}
}

impl ByteWriter for &mut [u8] {
	fn write(&mut self, bytes: &[u8]) {
		self.split_off_mut(..bytes.len())
			.unwrap()
			.copy_from_slice(bytes);
	}
}

impl LowLevelOp {
	fn to_bytes(self, bytes: &mut impl ByteWriter) {
		match self {
			Self::LoadBuffer(r, offset) => {
				let (mask1, mask2) = zmm_dest_bits(r);
				// vmovaps zmmA, [rcx+offset]
				bytes.write(&[0x62, 0xf1 ^ mask1, 0x7c, 0x48, 0x28, 0x81 | mask2]);
				bytes.write_u32(offset * 64);
			}
			Self::StoreBuffer(offset, r) => {
				let (mask1, mask2) = zmm_dest_bits(r);
				// vmovaps [rcx+offset], zmmA
				bytes.write(&[0x62, 0xf1 ^ mask1, 0x7c, 0x48, 0x29, 0x81 | mask2]);
				bytes.write_u32(offset * 64);
			}
			Self::Add(dest, src1, Location::Zmm(src2)) => {
				let (d1, d2) = zmm_dest_bits(dest);
				let (s11, s12) = zmm_src1_bits(src1);
				let (s21, s22) = zmm_src2_bits(src2);
				// vaddps zmmA, zmmB, zmmC
				bytes.write(&[
					0x62,
					0xf1 ^ d1 ^ s21,
					0x7c ^ s11,
					0x48 ^ s12,
					0x58,
					0xc0 ^ s22 ^ d2,
				]);
			}
			Self::Add(dest, src1, Location::Buffer(src2)) => {
				let (d1, d2) = zmm_dest_bits(dest);
				let (s11, s12) = zmm_src1_bits(src1);
				// vaddps zmmA, zmmB, [rcx+offset]
				bytes.write(&[0x62, 0xf1 ^ d1, 0x7c ^ s11, 0x48 ^ s12, 0x58, 0x81 ^ d2]);
				bytes.write_u32(src2 * 64);
			}
			Self::Add(dest, src1, Location::Constant(src2)) => {
				let (d1, d2) = zmm_dest_bits(dest);
				let (s11, s12) = zmm_src1_bits(src1);
				// vaddps zmmA, zmmB, [r8+offset]
				bytes.write(&[0x62, 0xd1 ^ d1, 0x7c ^ s11, 0x48 ^ s12, 0x58, 0x80 ^ d2]);
				bytes.write_u32(src2 * 64);
			}
			_ => todo!(),
		}
	}
}

#[allow(dead_code)]
fn print_disassembly(code: &[u8]) -> Result<(), Box<dyn Error>> {
	use std::process::Command;
	std::fs::write("a.out", code)?;
	Command::new("objdump")
		.args([
			"-w",
			"-b",
			"binary",
			"-Mintel,x86-64",
			"-m",
			"i386",
			"-D",
			"a.out",
		])
		.spawn()?
		.wait()?;
	Ok(())
}

fn try_main() -> Result<(), Box<dyn Error>> {
	let arg = std::env::args().nth(1);
	let filename = arg.unwrap_or("prospero.vm".into());
	let text = std::fs::read_to_string(&filename)
		.map_err(|e| format!("couldn't read prospero.vm: {e}"))?;
	let _ops = read_ops(text)?;
	const BIT_OFFSET: u32 = 2 + 4 * 3 + 4 + 2 * 4 + 2 * 3;
	let width: u16 = 4096;
	let height: u16 = 4096;
	if !width.is_multiple_of(16) {
		return Err(format!("width {width} should be a multiple of 16").into());
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
	let mut thread_count = available_parallelism().unwrap_or(16).next_power_of_two();
	while !height.is_multiple_of(thread_count) {
		thread_count >>= 1;
	}
	let pixels = PixelBuffer(unsafe { data.add(BIT_OFFSET as usize).cast() });
	let low_level_ops = [
		LowLevelOp::StoreBuffer(123, 1),
		LowLevelOp::LoadBuffer(12, 123),
		LowLevelOp::StoreBuffer(456, 2),
		LowLevelOp::Add(3, 12, Location::Constant(456)),
	];
	let mut core = vec![0u8; low_level_ops.len() * 16];
	let mut rest = &mut core[..];
	for op in low_level_ops.iter().copied() {
		op.to_bytes(&mut rest);
	}
	let rest_len = rest.len();
	core.truncate(core.len() - rest_len);
	if cfg!(debug_assertions) {
		print_disassembly(&core)?;
	}
	let code = unsafe { wrap_code(&core) }?;
	//let code = unsafe { wrap_code(include_asm!("test")) }?;
	let mut x_strides = [0.0; 16];
	for (i, x_stride) in x_strides.iter_mut().enumerate() {
		*x_stride = -1.0 + i as f32 * 2.0 / f32::from(width);
	}
	let rows_per_thread = height / thread_count;
	let x_strides = ZmmValue(x_strides);
	let info = Info {
		code,
		pixels,
		rows_per_thread,
		x_strides,
		width,
		height,
		constants: vec![ZmmValue::default(); 10_000], //TODO
	};
	if thread_count == 1 {
		unsafe { info.thread_main(0) };
	} else {
		std::thread::scope(|s| {
			let mut threads = vec![];
			for t in 0..thread_count {
				let info = &info;
				threads.push(s.spawn(move || unsafe { info.thread_main(t) }));
			}
			for thread in threads {
				thread.join().unwrap();
			}
		});
	}
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
