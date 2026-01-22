; do_row(
;     rdi = ptr to output data
;     rsi = count (pixels / 16)
;     rdx = ptr to [-1, -1+x_stride, -1+2*x_stride, .., -1+16*x_stride]
;     xmm0 = y position
;     xmm1 = 16*x_stride
; )
BITS 64
do_row:
mov eax, 0xbf800000
vbroadcastss zmm0, xmm0
vbroadcastss zmm1, xmm1
; zmm2 = x position
vmovdqa64 zmm2, [rdx]
.loop:
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
	; zmm3 = result
	vpmovd2m k1, zmm3
	kmovw eax, k1
	not eax
	mov [rdi], ax
	add rdi, 2
; increment x
	vaddps zmm2, zmm1
; decrement count
	dec rsi
	jg .loop
	ret
