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
