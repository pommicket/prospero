; do_row(
;     rdi = ptr to output data
;     rsi = number of groups of 16 pixels (width of image / 16)
;     rdx = ptr to [-1, -1+x_stride, -1+2*x_stride, .., -1+16*x_stride]
;           where x_stride = 2.0 / image_width
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
mov eax, 0xbf800000
vbroadcastss zmm0, xmm0
vbroadcastss zmm1, xmm1
; zmm2 = x position
vmovaps zmm2, [rdx]
.loop:
	marker

	; zmm3 = result

; unfortunately, BMP, unlike all other file formats on the planet,
; treats the most significant bit of a byte as bit 0
; so we have to shuffle around the values
; 27 = 0b00 01 10 11
	vpshufd zmm3, zmm3, 27

; extract sign bit from each float in zmm3. kind of a miracle that this exists!
	vpmovd2m k1, zmm3
	kmovw eax, k1
; reverse groups of 4 bits in each byte
	ror al, 4
	ror ah, 4

	mov [rdi], ax
	add rdi, 2
; increment x
	vaddps zmm2, zmm0
; decrement count
	dec rsi


