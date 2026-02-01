; do_row(
;     rdi = ptr to output data
;     rsi = number of groups of 16 pixels (width of image / 16)
;     rdx = ptr to [-1, -1+x_stride, -1+2*x_stride, .., -1+16*x_stride]
;           where x_stride = 2.0 / image_width
;           except reordered because BMP is weird
;     rcx = ptr to buffer to hold variables
;     r8 = ptr to constants
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
vbroadcastss zmm0, xmm0
vbroadcastss zmm1, xmm1
; zmm2 = x position
vmovaps zmm2, [rdx]

; main loop
	marker

	; zmm3 = result

; extract sign bit from each float in zmm3. kind of a miracle that this exists!
	vpmovd2m k1, zmm3
	kmovw eax, k1

	mov [rdi], ax
	add rdi, 2
; increment x
	vaddps zmm2, zmm0
; decrement count
	dec rsi


