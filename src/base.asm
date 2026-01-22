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
vbroadcastss zmm1, xmm1
vmovdqa64 zmm2, [rdx]
int3
int3
int3
int3
vaddps zmm2, zmm1
