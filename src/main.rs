use std::collections::{BinaryHeap, HashMap};
use std::error::Error;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(not(target_arch = "x86_64"))]
fn _check_target() {
	compile_error!("Only x86-64 target is supported.");
}

const SINGLE_THREADED: bool = false;
const ZMM_X: u8 = 2;
const ZMM_Y: u8 = 1;
const ZMM_SCRATCH: u8 = 3;
const ZMM_OUTPUT: u8 = 3;
// first zmm register free to use for storage
const ZMM_FREE: u8 = 4;

macro_rules! include_asm {
	($name:literal) => {
		include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".out"))
	};
}

/// High-level operation
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
	/// Subtract variable from constant
	///
	/// (for subtracting constant from variable, can just use AddConst with negative constant)
	SubConst(u16, f32),
	MaxConst(u16, f32),
	MinConst(u16, f32),
}

/// Parse .vm variable, e.g. `_f3c`
fn parse_var(var: &str) -> u16 {
	debug_assert!(var.starts_with('_'));
	u16::from_str_radix(&var[1..], 16).expect("bad variable")
}

/// Optimal # of threads to use
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

/// Buffer to hold the bitmap.
///
/// Essentially only exists so we can implement `Send` and `Sync` for it
#[derive(Copy, Clone)]
struct PixelBuffer(*mut u8);
impl PixelBuffer {
	/// Compute offsetted pixel buffer by certain number of bytes
	///
	/// # Safety
	/// `by` must be `<=` the size of the buffer in bytes.
	#[must_use]
	unsafe fn offset(self, by: usize) -> Self {
		Self(unsafe { self.0.add(by) })
	}
	/// Get pointer to pixel data
	fn ptr(self) -> *mut u8 {
		self.0
	}
}
unsafe impl Send for PixelBuffer {}
unsafe impl Sync for PixelBuffer {}

/// Holds pointer to JIT-created code.
///
/// Essentially only exists so we can implement `Send` and `Sync` for it
#[derive(Copy, Clone)]
struct Code(*mut u8);
unsafe impl Send for Code {}
unsafe impl Sync for Code {}
impl Code {
	/// Run the code. See `asm/base.asm` for more details about the parameters.
	///
	/// # Safety
	/// Code pointer must point to valid code, given the parameters.
	#[allow(clippy::too_many_arguments)]
	unsafe fn run(
		self,
		output_data: *mut u8,
		count: u16,
		x_strides: &ZmmValue,
		stride16: f32,
		y_pos: f32,
		buffer: &mut [ZmmValue],
		constants: &[f32],
	) {
		let function: unsafe extern "sysv64" fn(
			*mut u8,
			u64,
			*const f32,
			f32,
			f32,
			*mut ZmmValue,
			*const f32,
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

/// Wrap raw machine code with proper prologue and epilogue,
/// and map it to executable memory.
fn wrap_code(core: &[u8]) -> Result<Code, Box<dyn Error>> {
	let base_template = include_asm!("base");
	let marker_idx = base_template
		.windows(16)
		.position(|win| win == [0xcc; 16])
		.ok_or("bad ASM - no marker")?;
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
	// create jump at bottom of pixel loop with proper offset
	let epilogue = [0x0f, 0x8f, j0, j1, j2, j3, 0xc3];
	unsafe { ptr.copy_from_nonoverlapping(epilogue.as_ptr(), epilogue.len()) };
	let code_size = unsafe { ptr.offset_from(code) } as usize;
	let mut cpuinfo: u64 = 0;
	unsafe {
		std::arch::asm!(
			"push rbx\n
			cpuid\n
			mov {out}, rbx
			pop rbx",
			in("eax") 1,
			out = out(reg) cpuinfo,
			lateout("eax") _,
			out("ecx") _,
			out("edx") _,
		);
	};
	let cache_line_size = 8 * ((cpuinfo >> 8) & 0xff) as usize;
	// flush cache lines containing code to ensure it makes it out of the d-cache.
	for i in 0..code_size / cache_line_size {
		unsafe {
			std::arch::asm!(
				"clflushopt [{x}]",
				x = in(reg) code.add(i * 64)
			)
		}
	}
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
	fn constant(value: f32) -> Self {
		Self([value; 16])
	}
	fn map(self, func: impl Fn(usize, f32) -> f32) -> Self {
		let mut i = 0;
		Self(self.0.map(|x| {
			let value = func(i, x);
			i += 1;
			value
		}))
	}
	fn min(self, other: Self) -> Self {
		self.map(|i, x| x.min(other[i]))
	}
	fn max(self, other: Self) -> Self {
		self.map(|i, x| x.max(other[i]))
	}
	fn sqrt(self) -> Self {
		self.map(|_, x| x.sqrt())
	}
}

impl std::ops::Neg for ZmmValue {
	type Output = Self;
	fn neg(self) -> Self::Output {
		self.map(|_, x| -x)
	}
}

impl std::ops::Index<usize> for ZmmValue {
	type Output = f32;
	fn index(&self, index: usize) -> &Self::Output {
		&self.0[index]
	}
}

impl std::ops::Add for ZmmValue {
	type Output = ZmmValue;
	fn add(self, rhs: Self) -> Self::Output {
		self.map(|i, x| x + rhs[i])
	}
}

impl std::ops::Sub for ZmmValue {
	type Output = ZmmValue;
	fn sub(self, rhs: Self) -> Self::Output {
		self.map(|i, x| x - rhs[i])
	}
}

impl std::ops::Mul for ZmmValue {
	type Output = ZmmValue;
	fn mul(self, rhs: Self) -> Self::Output {
		self.map(|i, x| x * rhs[i])
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
	x_strides: ZmmValue,
	width: u16,
	height: u16,
	constants: Vec<f32>,
	instructions: Vec<Instruction>,
	buffer_entries_needed: u32,
	next_row: AtomicU32,
}

impl Info {
	fn get_value(&self, zmm: &[ZmmValue; 32], buffer: &[ZmmValue], location: Location) -> ZmmValue {
		match location {
			Location::Constant(c) => ZmmValue::constant(self.constants[c as usize]),
			Location::Zmm(z) => zmm[z as usize],
			Location::Buffer(b) => buffer[b as usize],
		}
	}
	fn interpret(
		&self,
		instruction: Instruction,
		zmm: &mut [ZmmValue; 32],
		buffer: &mut [ZmmValue],
	) {
		match instruction {
			Instruction::Add(a, b, c) => {
				zmm[a as usize] = zmm[b as usize] + self.get_value(zmm, buffer, c);
			}
			Instruction::Sub(a, b, c) => {
				zmm[a as usize] = zmm[b as usize] - self.get_value(zmm, buffer, c);
			}
			Instruction::Mul(a, b, c) => {
				zmm[a as usize] = zmm[b as usize] * self.get_value(zmm, buffer, c);
			}
			Instruction::Min(a, b, c) => {
				zmm[a as usize] = zmm[b as usize].min(self.get_value(zmm, buffer, c));
			}
			Instruction::Max(a, b, c) => {
				zmm[a as usize] = zmm[b as usize].max(self.get_value(zmm, buffer, c));
			}
			Instruction::Sqrt(a, b) => {
				zmm[a as usize] = self.get_value(zmm, buffer, b).sqrt();
			}
			Instruction::LoadBuffer(z, b) => {
				zmm[z as usize] = buffer[b as usize];
			}
			Instruction::StoreBuffer(b, z) => {
				buffer[b as usize] = zmm[z as usize];
			}
			Instruction::Negate(a, b) => {
				zmm[a as usize] = -zmm[b as usize];
			}
			Instruction::LoadConstant(a, b) => {
				zmm[a as usize] = ZmmValue::constant(self.constants[b as usize]);
			}
		}
	}
	unsafe fn thread_main(&self, _thread_idx: u16) {
		let interpreted = false; // for testing purposes
		let inv_width2 = 2.0 / f32::from(self.width);
		let inv_height2 = 2.0 / f32::from(self.height);
		let pixels = self.pixels;
		let width = self.width;
		let code = self.code;
		let x_strides = &self.x_strides;
		let mut buffer = vec![ZmmValue::default(); self.buffer_entries_needed as usize];
		let constants = &self.constants;
		loop {
			let y = self.next_row.fetch_add(1, Ordering::Relaxed);
			if y >= u32::from(self.height) {
				break;
			}
			let pixel = unsafe { pixels.offset(y as usize * usize::from(width) / 8) };
			let y = y as f32 * inv_height2 - 1.0;
			if interpreted {
				let mut zmm = [ZmmValue::default(); 32];
				let mut ptr = pixel.ptr();
				for x_idx in 0..self.width / 16 {
					let x = x_idx as f32 * 16.0 * inv_width2;
					zmm[ZMM_X as usize] = ZmmValue::constant(x) + *x_strides;
					zmm[ZMM_Y as usize] = ZmmValue::constant(y);
					for instruction in self.instructions.iter().copied() {
						self.interpret(instruction, &mut zmm, &mut buffer);
					}
					let output = zmm[ZMM_OUTPUT as usize];
					for i in 0..8 {
						if output.0[i] < 0.0 {
							unsafe {
								*ptr |= 1 << (7 - i);
							}
						}
					}
					ptr = unsafe { ptr.add(1) };
					for i in 8..16 {
						if output.0[i] < 0.0 {
							unsafe {
								*ptr |= 1 << (15 - i);
							}
						}
					}
					ptr = unsafe { ptr.add(1) };
				}
			} else {
				unsafe {
					code.run(
						pixel.ptr(),
						width / 16,
						x_strides,
						16.0 * inv_width2,
						y,
						&mut buffer,
						constants,
					)
				};
			}
		}
	}
}

fn read_ops(text: String) -> Result<Vec<Op>, Box<dyn Error>> {
	let mut ops = vec![];
	let mut index: u16 = 0;
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

impl Location {
	fn load_zmm(self, instructions: &mut Vec<Instruction>) -> u8 {
		match self {
			Self::Zmm(n) => n,
			Self::Buffer(x) => {
				instructions.push(Instruction::LoadBuffer(ZMM_SCRATCH, x));
				ZMM_SCRATCH
			}
			Self::Constant(x) => {
				instructions.push(Instruction::LoadConstant(ZMM_SCRATCH, x));
				ZMM_SCRATCH
			}
		}
	}
}

#[derive(Clone, Copy, Debug)]
enum Instruction {
	LoadBuffer(u8, u32),
	LoadConstant(u8, u32),
	StoreBuffer(u32, u8),
	Add(u8, u8, Location),
	Sub(u8, u8, Location),
	Mul(u8, u8, Location),
	Sqrt(u8, Location),
	Min(u8, u8, Location),
	Max(u8, u8, Location),
	Negate(u8, u8),
}

impl Instruction {
	fn output_register(&mut self) -> Option<&mut u8> {
		match self {
			Self::LoadBuffer(x, _) => Some(x),
			Self::LoadConstant(x, _) => Some(x),
			Self::StoreBuffer(..) => None,
			Self::Add(x, ..) => Some(x),
			Self::Sub(x, ..) => Some(x),
			Self::Mul(x, ..) => Some(x),
			Self::Min(x, ..) => Some(x),
			Self::Max(x, ..) => Some(x),
			Self::Negate(x, ..) => Some(x),
			Self::Sqrt(x, ..) => Some(x),
		}
	}
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

fn write_memory_operand(bytes: &mut impl ByteWriter, base: u8, offset: u32, sizeof: u32) {
	if offset < 128 {
		bytes.write(&[base ^ 0x40, offset as u8]);
	} else {
		bytes.write(&[base ^ 0x80]);
		bytes.write_u32(offset * sizeof);
	}
}

fn binary_op_to_bytes(bytes: &mut impl ByteWriter, op: u8, dest: u8, src1: u8, src2: Location) {
	match src2 {
		Location::Zmm(src2) => {
			let (d1, d2) = zmm_dest_bits(dest);
			let (s11, s12) = zmm_src1_bits(src1);
			let (s21, s22) = zmm_src2_bits(src2);
			// vXps zmmA, zmmB, zmmC
			bytes.write(&[
				0x62,
				0xf1 ^ d1 ^ s21,
				0x7c ^ s11,
				0x48 ^ s12,
				op,
				0xc0 ^ s22 ^ d2,
			]);
		}
		Location::Buffer(src2) => {
			let (d1, d2) = zmm_dest_bits(dest);
			let (s11, s12) = zmm_src1_bits(src1);
			// vXps zmmA, zmmB, [rcx+offset]
			bytes.write(&[0x62, 0xf1 ^ d1, 0x7c ^ s11, 0x48 ^ s12, op]);
			write_memory_operand(bytes, 0x01 | d2, src2, 64);
		}
		Location::Constant(src2) => {
			let (d1, d2) = zmm_dest_bits(dest);
			let (s11, s12) = zmm_src1_bits(src1);
			// vXps zmmA, zmmB, dword bcst [r8+offset]
			bytes.write(&[0x62, 0xd1 ^ d1, 0x7c ^ s11, 0x58 ^ s12, op]);
			write_memory_operand(bytes, d2, src2, 4);
		}
	}
}

impl Instruction {
	fn to_bytes(self, bytes: &mut impl ByteWriter) {
		match self {
			Self::LoadConstant(r, offset) => {
				let (mask1, mask2) = zmm_dest_bits(r);
				// vbroadcastss zmmA, [r8+offset]
				bytes.write(&[0x62, 0xd2 ^ mask1, 0x7d, 0x48, 0x18]);
				write_memory_operand(bytes, mask2, offset, 4);
			}
			Self::LoadBuffer(r, offset) => {
				let (mask1, mask2) = zmm_dest_bits(r);
				// vmovaps zmmA, [rcx+offset]
				bytes.write(&[0x62, 0xf1 ^ mask1, 0x7c, 0x48, 0x28]);
				write_memory_operand(bytes, mask2 | 0x01, offset, 64);
			}
			Self::StoreBuffer(offset, r) => {
				let (mask1, mask2) = zmm_dest_bits(r);
				// vmovaps [rcx+offset], zmmA
				bytes.write(&[0x62, 0xf1 ^ mask1, 0x7c, 0x48, 0x29]);
				write_memory_operand(bytes, mask2 | 0x01, offset, 64);
			}
			Self::Add(dest, src1, src2) => binary_op_to_bytes(bytes, 0x58, dest, src1, src2),
			Self::Sub(dest, src1, src2) => binary_op_to_bytes(bytes, 0x5c, dest, src1, src2),
			Self::Mul(dest, src1, src2) => binary_op_to_bytes(bytes, 0x59, dest, src1, src2),
			Self::Min(dest, src1, src2) => binary_op_to_bytes(bytes, 0x5d, dest, src1, src2),
			Self::Max(dest, src1, src2) => binary_op_to_bytes(bytes, 0x5f, dest, src1, src2),
			Self::Sqrt(dest, Location::Zmm(src)) => {
				let (d1, d2) = zmm_dest_bits(dest);
				let (s1, s2) = zmm_src2_bits(src);
				// vsqrtps zmmA, zmmB
				bytes.write(&[0x62, 0xf1 ^ d1 ^ s1, 0x7c, 0x48, 0x51, 0xc0 ^ d2 ^ s2]);
			}
			Self::Sqrt(dest, Location::Buffer(src)) => {
				let (d1, d2) = zmm_dest_bits(dest);
				// vsqrtps zmmA, [rcx+offset]
				bytes.write(&[0x62, 0xf1 ^ d1, 0x7c, 0x48, 0x51]);
				write_memory_operand(bytes, d2 | 0x01, src, 64);
			}
			Self::Sqrt(_, Location::Constant(_)) => {
				panic!("shouldn't be taking the sqrt of a constant");
			}
			Self::Negate(dest, src) => {
				// vxorps zmmA, zmmB, dword bcst [r8]
				let (d1, d2) = zmm_dest_bits(dest);
				let (s1, s2) = zmm_src1_bits(src);
				bytes.write(&[0x62, 0xd1 ^ d1, 0x7c ^ s1, 0x58 ^ s2, 0x57, d2]);
			}
		}
	}
}

fn print_disassembly(code: &[u8]) -> Result<(), Box<dyn Error>> {
	use std::process::Command;
	std::fs::write("a.out", code)?;
	let output = Command::new("objdump")
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
		.output()?;
	let stdout = String::from_utf8_lossy(&output.stdout);
	std::fs::write("disassembly.out", stdout.as_bytes())?;
	//println!("{stdout}");

	Ok(())
}
fn print_instructions(instructions: &[Instruction]) -> Result<(), Box<dyn Error>> {
	let out = std::fs::File::create("instructions.out")
		.map_err(|e| format!("error creating instructions.out: {e}"))?;
	let mut out = std::io::BufWriter::new(out);
	for instruction in instructions {
		writeln!(out, "{instruction:?}")?;
	}
	Ok(())
}

struct CompilationResult {
	constants: Vec<f32>,
	instructions: Vec<Instruction>,
	buffer_entries_needed: u32,
}

#[derive(Default)]
struct ConstantList {
	array: Vec<f32>,
	map: HashMap<u32, u32>,
}

impl ConstantList {
	fn add(&mut self, constant: f32) -> u32 {
		// must ensure 4*constant_id is a valid immediate 32-bit offset
		assert!(self.array.len() < (1 << 29));
		if let Some(id) = self.map.get(&constant.to_bits()).copied() {
			return id;
		}
		let id = self.array.len() as u32;
		self.array.push(constant);
		self.map.insert(constant.to_bits(), id);
		id
	}
}

struct BufferUser {
	last_use: u16,
	buffer_idx: u32,
}

impl PartialEq for BufferUser {
	fn eq(&self, other: &Self) -> bool {
		matches!(self.cmp(other), std::cmp::Ordering::Equal)
	}
}

impl Eq for BufferUser {}

impl Ord for BufferUser {
	fn cmp(&self, other: &Self) -> std::cmp::Ordering {
		other.last_use.cmp(&self.last_use)
	}
}

impl PartialOrd for BufferUser {
	fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

struct Compiler {
	buffer_idx: u32,
	op_idx: u16,
	uses: Vec<Vec<u16>>,
	instructions: Vec<Instruction>,
	locations: Vec<Location>,
	zmm_users: [Option<u16>; 32],
	buffer_users: BinaryHeap<BufferUser>,
	constants: ConstantList,
}

impl Compiler {
	fn last_use(&self, op: u16) -> u16 {
		self.uses[op as usize].last().copied().unwrap_or(0)
	}
	fn next_use_from(&self, from: u16, op: u16) -> u16 {
		let uses = &self.uses[op as usize];
		let next_use = uses.binary_search(&from).unwrap_or_else(|x| x);
		uses[next_use]
	}
	fn next_use(&self, op: u16) -> u16 {
		self.next_use_from(self.op_idx, op)
	}
	fn allocate_zmm(&mut self) -> u8 {
		for zmm in ZMM_FREE..=31 {
			let user = self.zmm_users[zmm as usize];
			let Some(user) = user else {
				self.zmm_users[zmm as usize] = Some(self.op_idx);
				return zmm;
			};
			if self.last_use(user) < self.op_idx {
				self.zmm_users[zmm as usize] = Some(self.op_idx);
				return zmm;
			}
		}
		let zmm = (ZMM_FREE..=31)
			.max_by_key(|&x| self.next_use(self.zmm_users[x as usize].unwrap()))
			.unwrap();
		// evict previous user
		let prev_user = self.zmm_users[zmm as usize].unwrap();
		let buffer_idx = if let Some(prev_buffer_user) = self.buffer_users.peek()
			&& prev_buffer_user.last_use < self.op_idx
		{
			// use this old buffer slot instead of allocating a new one
			let idx = prev_buffer_user.buffer_idx;
			self.buffer_users.pop();
			idx
		} else {
			let idx = self.buffer_idx;
			self.buffer_idx += 1;
			let last_use = self.last_use(prev_user);
			self.buffer_users.push(BufferUser {
				buffer_idx: idx,
				last_use,
			});
			idx
		};
		self.instructions
			.push(Instruction::StoreBuffer(buffer_idx, zmm));
		self.locations[prev_user as usize] = Location::Buffer(buffer_idx);
		self.zmm_users[zmm as usize] = Some(self.op_idx);
		zmm
	}
	#[must_use]
	fn compile_binop_with_constant(
		&mut self,
		constructor: impl FnOnce(u8, u8, Location, &mut Vec<Instruction>),
		arg: Location,
		constant: u32,
	) -> Location {
		let dest = self.allocate_zmm();
		let zmm = arg.load_zmm(&mut self.instructions);
		constructor(
			dest,
			zmm,
			Location::Constant(constant),
			&mut self.instructions,
		);
		Location::Zmm(dest)
	}
	#[must_use]
	fn compile_binop(
		&mut self,
		constructor: impl FnOnce(u8, u8, Location) -> Instruction,
		arg1: Location,
		arg2: Location,
	) -> Location {
		let dest = self.allocate_zmm();
		let zmm1 = arg1.load_zmm(&mut self.instructions);
		self.instructions.push(constructor(dest, zmm1, arg2));
		Location::Zmm(dest)
	}
	#[must_use]
	fn compile_unary(
		&mut self,
		constructor: impl FnOnce(u8, Location) -> Instruction,
		arg: Location,
	) -> Location {
		let dest = self.allocate_zmm();
		self.instructions.push(constructor(dest, arg));
		Location::Zmm(dest)
	}

	fn compile_op(&mut self, op: Op) -> Location {
		match op {
			Op::VarX => Location::Zmm(ZMM_X),
			Op::VarY => Location::Zmm(ZMM_Y),
			Op::AddConst(arg, constant) => {
				let constant = self.constants.add(constant);
				let arg = self.locations[arg as usize];
				self.compile_binop_with_constant(
					|d, s1, s2, instructions| {
						instructions.push(Instruction::Add(d, s1, s2));
					},
					arg,
					constant,
				)
			}
			Op::MulConst(arg, constant) => {
				let constant = self.constants.add(constant);
				let arg = self.locations[arg as usize];
				self.compile_binop_with_constant(
					|d, s1, s2, instructions| {
						instructions.push(Instruction::Mul(d, s1, s2));
					},
					arg,
					constant,
				)
			}
			Op::MinConst(arg, constant) => {
				let constant = self.constants.add(constant);
				let arg = self.locations[arg as usize];
				self.compile_binop_with_constant(
					|d, s1, s2, instructions| {
						instructions.push(Instruction::Min(d, s1, s2));
					},
					arg,
					constant,
				)
			}
			Op::MaxConst(arg, constant) => {
				let constant = self.constants.add(constant);
				let arg = self.locations[arg as usize];
				self.compile_binop_with_constant(
					|d, s1, s2, instructions| {
						instructions.push(Instruction::Max(d, s1, s2));
					},
					arg,
					constant,
				)
			}
			Op::SubConst(arg, constant) => {
				if constant == 0.0 {
					// special case: negate
					let arg = self.locations[arg as usize];
					let dest = self.allocate_zmm();
					let arg = arg.load_zmm(&mut self.instructions);
					self.instructions.push(Instruction::Negate(dest, arg));
					Location::Zmm(dest)
				} else {
					let constant = self.constants.add(constant);
					let arg = self.locations[arg as usize];
					let dest = self.allocate_zmm();
					self.instructions
						.push(Instruction::LoadConstant(ZMM_SCRATCH, constant));
					self.instructions
						.push(Instruction::Sub(dest, ZMM_SCRATCH, arg));
					Location::Zmm(dest)
				}
			}
			Op::Add(arg1, arg2) => {
				let arg1 = self.locations[arg1 as usize];
				let arg2 = self.locations[arg2 as usize];
				self.compile_binop(Instruction::Add, arg1, arg2)
			}
			Op::Sub(arg1, arg2) => {
				let arg1 = self.locations[arg1 as usize];
				let arg2 = self.locations[arg2 as usize];
				self.compile_binop(Instruction::Sub, arg1, arg2)
			}
			Op::Min(arg1, arg2) => {
				let arg1 = self.locations[arg1 as usize];
				let arg2 = self.locations[arg2 as usize];
				self.compile_binop(Instruction::Min, arg1, arg2)
			}
			Op::Max(arg1, arg2) => {
				let arg1 = self.locations[arg1 as usize];
				let arg2 = self.locations[arg2 as usize];
				self.compile_binop(Instruction::Max, arg1, arg2)
			}
			Op::Mul(arg1, arg2) => {
				let arg1 = self.locations[arg1 as usize];
				let arg2 = self.locations[arg2 as usize];
				self.compile_binop(Instruction::Mul, arg1, arg2)
			}
			Op::Sqrt(arg) => {
				let arg = self.locations[arg as usize];
				self.compile_unary(Instruction::Sqrt, arg)
			}
		}
	}
}

// uses[i] = list of ops which use the value of op #i
fn get_uses(ops: &[Op]) -> Vec<Vec<u16>> {
	let mut uses = vec![vec![]; ops.len()];
	for (i, op) in ops.iter().copied().enumerate() {
		let i = i as u16;
		let mut update = |arg: u16| uses[arg as usize].push(i);
		match op {
			Op::VarX | Op::VarY => {}
			Op::Sqrt(x)
			| Op::AddConst(x, _)
			| Op::SubConst(x, _)
			| Op::MulConst(x, _)
			| Op::MinConst(x, _)
			| Op::MaxConst(x, _) => update(x),
			Op::Add(x, y) | Op::Sub(x, y) | Op::Mul(x, y) | Op::Min(x, y) | Op::Max(x, y) => {
				update(x);
				update(y);
			}
		}
	}
	uses
}

fn compile_down(ops: Vec<Op>) -> CompilationResult {
	let mut constants = ConstantList::default();
	constants.add(f32::from_bits(0x8000_0000));
	let uses = get_uses(&ops);
	let mut compiler = Compiler {
		instructions: vec![],
		locations: vec![],
		buffer_idx: 0,
		uses,
		zmm_users: [None; 32],
		op_idx: 0,
		buffer_users: Default::default(),
		constants,
	};
	for (i, op) in ops.iter().copied().enumerate() {
		compiler.op_idx = i as u16;
		let location = compiler.compile_op(op);
		compiler.locations.push(location);
	}
	let Compiler {
		mut instructions,
		locations,
		buffer_idx,
		constants,
		..
	} = compiler;
	match *locations.last().unwrap() {
		Location::Buffer(b) => {
			instructions.push(Instruction::LoadBuffer(ZMM_OUTPUT, b));
		}
		Location::Constant(c) => {
			instructions.push(Instruction::LoadConstant(ZMM_OUTPUT, c));
		}
		Location::Zmm(z) => {
			if let Some(output) = instructions
				.last_mut()
				.and_then(|i| i.output_register())
				.filter(|x| **x == z)
			{
				// fix up output register of last instruction
				*output = ZMM_OUTPUT;
			} else {
				// otherwise,
				// (weird case, only happens with extraneous instructions
				//  or result = var-x or something)
				// do this suboptimal register transfer
				instructions.push(Instruction::StoreBuffer(0, z));
				instructions.push(Instruction::LoadBuffer(ZMM_OUTPUT, 0));
			}
		}
	}
	CompilationResult {
		constants: constants.array,
		instructions,
		buffer_entries_needed: buffer_idx.max(1),
	}
}

fn try_main() -> Result<(), Box<dyn Error>> {
	// needed for vpmovd2m
	if !is_x86_feature_detected!("avx512dq") {
		return Err("Your CPU doesn't support AVX512DQ. Sorry ):".into());
	}
	let arg = std::env::args().nth(1);
	let filename = arg.unwrap_or("prospero.vm".into());
	let text = std::fs::read_to_string(&filename)
		.map_err(|e| format!("couldn't read prospero.vm: {e}"))?;
	let ops = read_ops(text)?;
	const BIT_OFFSET: u32 = 2 + 4 * 3 + 4 + 2 * 4 + 2 * 3;
	let width: u16 = 1024;
	let height: u16 = 1024;
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
	let thread_count: u16 = available_parallelism().unwrap_or(16).min(height);
	let pixels = PixelBuffer(unsafe { data.add(BIT_OFFSET as usize).cast() });
	let CompilationResult {
		instructions,
		constants,
		buffer_entries_needed,
	} = compile_down(ops);
	if cfg!(debug_assertions) {
		print_instructions(&instructions)?;
		println!("{}KB of buffer space needed", buffer_entries_needed / 16);
		print!("{} instructions emitted ", instructions.len());
	}
	let mut core = vec![0u8; instructions.len() * 16];
	let mut rest = &mut core[..];
	for op in instructions.iter().copied() {
		op.to_bytes(&mut rest);
	}
	let rest_len = rest.len();
	core.truncate(core.len() - rest_len);
	if cfg!(debug_assertions) {
		println!("({} bytes)", core.len());
		print_disassembly(&core)?;
	}
	let code = wrap_code(&core)?;
	//let code = wrap_code(include_asm!("test"))?;
	let mut x_strides = [0.0; 16];
	for (i, x_stride) in x_strides.iter_mut().enumerate() {
		*x_stride = -1.0 + i as f32 * 2.0 / f32::from(width);
	}
	let x_strides = ZmmValue(x_strides);
	let info = Info {
		code,
		pixels,
		x_strides,
		width,
		height,
		constants,
		instructions,
		buffer_entries_needed,
		next_row: AtomicU32::new(0),
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
	_ = unsafe { libc::munmap(data.cast(), file_size as usize) };
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
