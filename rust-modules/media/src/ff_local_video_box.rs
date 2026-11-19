//! Rounded 2x2 box average for subsampled video planes.
pub(super) struct BoxFilter {
    #[cfg(all(target_arch = "arm", target_os = "linux"))]
    neon: bool,
}

impl BoxFilter {
    #[inline]
    pub(super) fn detect() -> Self {
        Self {
            #[cfg(all(target_arch = "arm", target_os = "linux"))]
            neon: arm::available(),
        }
    }

    #[inline]
    pub(super) fn row(&self, top: &[u8], bottom: &[u8], dst: &mut [u8]) {
        debug_assert_eq!(top.len(), dst.len() * 2);
        debug_assert_eq!(bottom.len(), top.len());
        #[cfg(all(target_arch = "arm", target_os = "linux"))]
        let complete = if self.neon {
            // Bounded from all three slices even if a caller supplies a shorter row.
            let groups = (dst.len() / 8).min(top.len() / 16).min(bottom.len() / 16);
            if groups != 0 {
                unsafe {
                    arm::ott_grid_box8(dst.as_mut_ptr(), top.as_ptr(), bottom.as_ptr(), groups)
                };
            }
            groups * 8
        } else {
            0
        };
        #[cfg(not(all(target_arch = "arm", target_os = "linux")))]
        let complete = 0;
        let top = top[complete * 2..].chunks_exact(2);
        let bottom = bottom[complete * 2..].chunks_exact(2);
        for ((pixel, a), b) in dst[complete..].iter_mut().zip(top).zip(bottom) {
            let sum = u16::from(a[0]) + u16::from(a[1]) + u16::from(b[0]) + u16::from(b[1]);
            *pixel = ((sum + 2) / 4) as u8;
        }
    }
}

#[cfg(all(target_arch = "arm", target_os = "linux"))]
mod arm {
    use std::sync::OnceLock;
    core::arch::global_asm!(include_str!("ff_local_video_box_arm.S"), options(raw));
    unsafe extern "C" {
        pub(super) fn ott_grid_box8(dst: *mut u8, top: *const u8, bottom: *const u8, groups: usize);
    }
    pub(super) fn available() -> bool {
        // Linux ARM32 asm/hwcap.h. The app already ships the glibc-baseline getauxval fallback.
        const HWCAP_NEON: libc::c_ulong = 1 << 12;
        static NEON: OnceLock<bool> = OnceLock::new();
        *NEON.get_or_init(|| unsafe { libc::getauxval(libc::AT_HWCAP) } & HWCAP_NEON != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::BoxFilter;

    #[test]
    fn unaligned_rows_and_partial_vector_match_integer_reference() {
        let bytes: Vec<u8> = (0..240).map(|i| (i * 53 + i * i) as u8).collect();
        let filter = BoxFilter::detect();
        for offset in 0..4 {
            for width in 1..=39 {
                let a = &bytes[offset..offset + width * 2];
                let b = &bytes[93 + offset..93 + offset + width * 2];
                let mut dst = vec![0xff; width + 2];
                filter.row(a, b, &mut dst[1..=width]);
                assert_eq!(dst[0], 0xff);
                assert_eq!(dst[width + 1], 0xff);
                for i in 0..width {
                    let sum = [a[2 * i], a[2 * i + 1], b[2 * i], b[2 * i + 1]]
                        .map(u16::from)
                        .iter()
                        .sum::<u16>();
                    assert_eq!(dst[i + 1], ((sum + 2) / 4) as u8);
                }
            }
        }
    }
}
