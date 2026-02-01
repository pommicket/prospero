# prospero

Implementation of the [prospero challenge](https://www.mattkeeter.com/projects/prospero/)
using an AVX-512 JIT compiler.
This is a program that turns a list of mathematical operations
into x86-64 machine code using AVX-512 instructions to process 16 (32-bit FP) inputs in parallel.
This program does everything from reading the input `prospero.vm` file to writing an output
Microsoft® BMP image; all these steps from start to finish are included in the benchmark below.

Build &amp; run with:

```
cargo r --release
```

or

```
cargo r --release -- <vm file> <resolution>
```


Take out the `--release` to run in debug mode —
you'll also get some statistics and a `disassembly.out` file
with all the instructions produced by the JIT compiler.

## What?

See [the prospero challenge web site](https://www.mattkeeter.com/projects/prospero/) for
some background.

Over the years, a larger and larger proportion
of the time CPUs spend executing instructions has been devoted
to fetching, decoding, register renaming, scheduling, &amp;c.,
while very little time is needed to actually do the computation.
This has led CPU vendors to develop new instruction sets which let you do very sophisticated
computations all in a single instruction.
 
SSE was the first good instruction set for floating-point math on x86 CPUs.
It originally had 8 128-bit registers, which each could process four 32-bit floats at once.
When x86-64 came out, this was expanded to 16 registers. Then AVX extended them to 256 bits,
allowing eight 32-bit floats to be processed at once. Finally, AVX-512 increased the register
count to 32, and expanded them to 512 bits, meaning each register can hold *sixteen* 32-bit floats
which can be processed in parallel.
For example

```
vsqrtps zmm1, zmm2
```

Takes the square root of <i>all 16 single-precision floating-point numbers</i> 
in the `zmm2` register, and stores them in the `zmm1` register — all in a single instruction!

Unfortunately, AVX-512 is not supported on all that many CPUs, and this program
actually needs the DQ extension as well, so there is a very good chance that
it won't run on your computer.

## Benchmark

These benchmarks were performed on an AMD Ryzen® 5 7640U with 6 cores, 12 threads.
Performance should scale linearly with parallelism, and this is a laptop CPU, so it might
not have the best performance per core.

| Resolution | Time    |
| ---------- | ------- |
| 1024x1024  | 36ms    |
| 2048x2048  | 115ms   |
| 4096x4096  | 443ms   |
| 8192x8192  | 1719ms  |

You might be surprised to see that the performance gap between 1024x1024 and 2048x2048
is only ~3x instead of 4x. I believe the reason for this is just that it takes
time for the microarchitecture to adapt to the code being run, and there isn't enough
time to do that for 1024x1024. Indeed, running it a bunch of times in a row brings
the median time down to 21ms.

## Fun stuff discovered along the way

AVX-512 added "broadcasted" memory operands. So you have

```
# Adds rax[0] to zmm0[0], rax[1] to zmm0[1], rax[2] to zmm0[2], etc.
vaddps zmm0, zmm0, [rax]
# Adds rax[0] to zmm0[0], rax[0] to zmm0[1], rax[0] to zmm0[2], etc.
vaddps zmm0, zmm0, dword bcst [rax]
```

This is really useful for constants — back in the days of SSE/AVX, constants
had to be repeated a buncha times in memory!

(Funnily enough, GNU assembler will emit `[rax]` if you just type
`bcst [rax]` — or for that matter `meowmeow [rax]` — without any kind of warning or error.
What a delightful program!)


Did you know! The 8-bit immediate address offsets in AVX-512 instructions are multiplied
by the size of the operands (32 for zmmword and 4 for broadcasted float). This is nice because
way more offsets are encodable with a single byte — and unaligned offsets are definitely rare enough
to deserve the longer encoding — but it confused me at first! As far as
I know, no other x86-64 memory offsets get multiplied in this way.

Did you know! Microsoft® BMP stores bits starting from the *most significant bit* for &lt;8bpp formats!
Nobody but Microsoft could be innovative enough to consider bit 0 to be the most significant bit.

## Room for improvement

- Right now we only use source memory operands. Maybe there are cases where
  destination memory operands would be useful (although there aren't *too*
  many computations whose results are not used soon after, so I doubt there's
  much performance to be gained there).
- Better register allocation. Famously optimal register allocation is NP-complete
  — my strategy was to evict the value which will be next used most distantly in the
  future. Values which are evicted are never brought back into registers
  (except into the "scratch" zmm3 register temporarily), which is not ideal.
- Mathematical simplification. I don't know how well-simplified the `propsero.vm`
  expression is, so I don't know how much performance can be gained there.
- FMA instructions. I tried this out and didn't see much improvement,
  but maybe with more simplification and more complicated FMA detection it could work.
- Half-precision floats. You could process 32 pixels in parallel, but
  I suspect the accumulated error would be noticeable even at 1024x1024,
  and the resolution would effectively max out at 2048x2048 (beyond that, there would be
  adjacent pixels with the same half-precision coordinates).
- Offset the `rcx` and `r8` registers so more addresses can be accessed with the shorter
  signed 8-bit immediate offset encoding. I doubt this would make a big difference.
- Optimizing the expression for particular intervals of input values.
  This is what a lot of the better prospero submissions do, but I haven't really felt like doing
  it yet.
