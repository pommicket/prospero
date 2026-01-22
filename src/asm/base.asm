; do_row(
;     rdi = ptr to output data
;     rsi = count (pixels / 16)
;     rdx = ptr to [-1, -1+x_stride, -1+2*x_stride, .., -1+16*x_stride]
;     xmm0 = 16*x_stride
;     xmm1 = y position
; )
BITS 64
%macro marker 0
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
	int3
%endmacro
do_row:
mov eax, 0xbf800000
vbroadcastss zmm0, xmm0
vbroadcastss zmm1, xmm1
; zmm2 = x position
vmovdqa64 zmm2, [rdx]
.loop:
	marker
	; zmm3 = result
	vpmovd2m k1, zmm3
	kmovw eax, k1
	not eax

; unfortunately, BMP, unlike all other file formats on the planet,
; treats the most significant bit of a byte as bit 0
; so we have to swap around the bits in each byte
; compiled from Rust code:
;   pub fn foo(num: u16) -> u16 {
;       num.swap_bytes().reverse_bits()
;   }
	push rdi
	push rcx
        mov     edi, eax
        and     eax, 3855
        shl     eax, 4
        shr     edi, 4
        and     edi, 3855
        or      edi, eax
        mov     eax, edi
        and     eax, 13107
        shr     edi, 2
        and     edi, 13107
        lea     eax, [rdi + 4*rax]
        mov     ecx, eax
        and     ecx, 21845
        shr     eax, 1
        and     eax, 21845
        lea     eax, [rax + 2*rcx]
        pop rcx
        pop rdi


	mov [rdi], ax
	add rdi, 2
; increment x
	vaddps zmm2, zmm0
; decrement count
	dec rsi


