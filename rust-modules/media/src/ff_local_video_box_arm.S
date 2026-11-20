// Integer 2x2 box filter for ARMv7 CPUs advertising HWCAP_NEON.
// AAPCS r0 = destination, r1/r2 = source rows, r3 = number of eight-pixel groups.
// All memory accesses have byte alignment. Only caller-saved registers are used.
.syntax unified
.pushsection .text.ott_grid_box8,"ax",%progbits
.arm
.fpu neon
.balign 4
.global ott_grid_box8
.hidden ott_grid_box8
.type ott_grid_box8, %function
ott_grid_box8:
1:
    vld1.8      {q0}, [r1]!
    vld1.8      {q1}, [r2]!
    vpaddl.u8   q0, q0
    vpadal.u8   q0, q1
    vrshrn.u16  d0, q0, #2
    vst1.8      {d0}, [r0]!
    subs        r3, r3, #1
    bne         1b
    bx          lr
.size ott_grid_box8, .-ott_grid_box8
.popsection

// Eight adjacent 3x3 averages. vld3 separates each row into columns 0, 1 and 2.
// For each possible sum 0..2295, round(sum * 3641 / 32768) == (sum + 4) / 9.
// 3641 = ceil(32768/9); its rounding error stays below 1/18 (nearest half-step).
.pushsection .text.ott_grid_box3x8,"ax",%progbits
.balign 4
.global ott_grid_box3x8
.hidden ott_grid_box3x8
.type ott_grid_box3x8, %function
ott_grid_box3x8:
    movw        ip, #3641
    vdup.16     q11, ip
    ldr         ip, [sp]
1:
    vld3.8      {d0, d1, d2}, [r1]!
    vaddl.u8    q8, d0, d1
    vaddw.u8    q8, q8, d2
    vld3.8      {d3, d4, d5}, [r2]!
    vaddw.u8    q8, q8, d3
    vaddw.u8    q8, q8, d4
    vaddw.u8    q8, q8, d5
    vld3.8      {d0, d1, d2}, [r3]!
    vaddw.u8    q8, q8, d0
    vaddw.u8    q8, q8, d1
    vaddw.u8    q8, q8, d2
    vqrdmulh.s16 q8, q8, q11
    vmovn.u16   d0, q8
    vst1.8      {d0}, [r0]!
    subs        ip, ip, #1
    bne         1b
    bx          lr
.size ott_grid_box3x8, .-ott_grid_box3x8
.popsection
