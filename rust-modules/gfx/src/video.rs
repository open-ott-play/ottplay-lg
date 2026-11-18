//! Main-thread streaming planar video. This module owns pixels, never sources,
//! media clocks or native decoder resources. Drop before destroying its GL context.
use super::*;
use plx_base::task::MainThread;
use std::{marker::PhantomData, rc::Rc};

const LUMA: u32 = 0x1909;
const RED: u32 = 0x1903;
const MAX_TEXTURE: u32 = 0x0D33;
const MAX_UNITS: u32 = 0x8872;
const ACTIVE_TEXTURE: u32 = 0x84E0;
const TEXTURE_BINDING: u32 = 0x8069;
const CURRENT_PROGRAM: u32 = 0x8B8D;
const MAX_BYTES: usize = 32 * 1024 * 1024;
extern "C" {
    fn glGetIntegerv(name: u32, value: *mut i32);
    fn glTexSubImage2D(
        target: u32,
        level: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        format: u32,
        ty: u32,
        data: *const c_void,
    );
    fn glDeleteProgram(program: u32);
    fn glDeleteShader(shader: u32);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    Limited,
    Full,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_dimension: u32,
    pub texture_units: u32,
}
fn geometry(w: u32, h: u32, cw: u32, ch: u32, limit: u32) -> Result<[usize; 3], &'static str> {
    if w == 0
        || h == 0
        || w > limit
        || h > limit
        || w > 4096
        || h > 4096
        || ![(w, h), (w.div_ceil(2), h), (w.div_ceil(2), h.div_ceil(2))].contains(&(cw, ch))
    {
        return Err("Unsupported planar video dimensions");
    }
    let y = (w as usize)
        .checked_mul(h as usize)
        .ok_or("Video plane overflow")?;
    let c = (cw as usize)
        .checked_mul(ch as usize)
        .ok_or("Video plane overflow")?;
    if c.checked_mul(2)
        .and_then(|n| n.checked_add(y))
        .is_none_or(|n| n > MAX_BYTES)
    {
        return Err("Video frame exceeds pixel budget");
    }
    Ok([y, c, c])
}
fn validate_planes(
    y: usize,
    u: usize,
    v: usize,
    w: u32,
    h: u32,
    cw: u32,
    ch: u32,
    limit: u32,
) -> Result<(), &'static str> {
    if [y, u, v] != geometry(w, h, cw, ch, limit)? {
        return Err("Video plane length mismatch");
    }
    Ok(())
}
fn coefficients(range: Range, matrix: Matrix) -> ([f32; 3], [f32; 3]) {
    let scale = if range == Range::Limited {
        [255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0]
    } else {
        [1.0, 1.0, 0.0]
    };
    let coeff = match matrix {
        Matrix::Bt601 => [1.402, -0.344136, -0.714136],
        Matrix::Bt709 => [1.5748, -0.187324, -0.468124],
    };
    (scale, coeff)
}
/// Restores touched texture units (including the renderer's resident dither
/// binding), unpack alignment and cached program. Never changes blend/scissor/VBO.
struct State {
    active: i32,
    bindings: [i32; 3],
    unpack: i32,
    program: i32,
}
impl State {
    unsafe fn save() -> Self {
        let mut s = Self {
            active: 0,
            bindings: [0; 3],
            unpack: 0,
            program: 0,
        };
        glGetIntegerv(ACTIVE_TEXTURE, &mut s.active);
        glGetIntegerv(GL_UNPACK_ALIGNMENT, &mut s.unpack);
        glGetIntegerv(CURRENT_PROGRAM, &mut s.program);
        for i in 0..3 {
            glActiveTexture(GL_TEXTURE0 + i as u32);
            glGetIntegerv(TEXTURE_BINDING, &mut s.bindings[i]);
        }
        s
    }
}
impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            for i in 0..3 {
                glActiveTexture(GL_TEXTURE0 + i as u32);
                glBindTexture(GL_TEXTURE_2D, self.bindings[i] as u32);
            }
            glActiveTexture(self.active as u32);
            glPixelStorei(GL_UNPACK_ALIGNMENT, self.unpack);
            use_prog(self.program as u32);
        }
    }
}
fn video_program(fragment: *const c_char) -> Option<u32> {
    unsafe {
        let vertex = try_compile(GL_VERTEX_SHADER, VS_IMG.as_ptr())?;
        let Some(fragment) = try_compile(GL_FRAGMENT_SHADER, fragment) else {
            glDeleteShader(vertex);
            return None;
        };
        let program = glCreateProgram();
        glAttachShader(program, vertex);
        glAttachShader(program, fragment);
        glBindAttribLocation(program, 0, c"a_pos".as_ptr());
        glLinkProgram(program);
        let mut linked = 0;
        glGetProgramiv(program, GL_LINK_STATUS, &mut linked);
        glDeleteShader(vertex);
        glDeleteShader(fragment);
        if linked == 0 {
            glDeleteProgram(program);
            None
        } else {
            Some(program)
        }
    }
}
type Context = unsafe extern "C" fn() -> *mut c_void;
fn current_context() -> Option<Context> {
    let raw = plx_base::dynlib::Handle::self_handle().sym("SDL_GL_GetCurrentContext")?;
    if raw.is_null() {
        return None;
    }
    Some(unsafe { std::mem::transmute::<*mut c_void, Context>(raw) })
}
/// !Send/!Sync keeps uploads, draws and destruction on the creating UI thread.
pub struct Texture {
    ids: [u32; 3],
    program: u32,
    context: *mut c_void,
    context_fn: Context,
    sizes: Option<(u32, u32, u32, u32)>,
    limits: Limits,
    format: u32,
    range: Range,
    matrix: Matrix,
    locations: [i32; 10],
    _main: PhantomData<Rc<()>>,
}
impl Texture {
    pub fn probe(_main: &MainThread) -> Result<Limits, &'static str> {
        unsafe {
            if glGetString(GL_SHADING_LANGUAGE_VERSION).is_null() {
                return Err("No current graphics context");
            }
            let (mut dim, mut units) = (0, 0);
            glGetIntegerv(MAX_TEXTURE, &mut dim);
            glGetIntegerv(MAX_UNITS, &mut units);
            if dim <= 0 || units < 3 {
                return Err("Planar video texture resources unavailable");
            }
            Ok(Limits {
                max_dimension: dim as u32,
                texture_units: units as u32,
            })
        }
    }
    pub fn new(main: &MainThread) -> Result<Self, &'static str> {
        let limits = Self::probe(main)?;
        let context_fn = current_context().ok_or("Graphics context identity unavailable")?;
        let context = unsafe { context_fn() };
        if context.is_null() {
            return Err("No current graphics context");
        }
        let program=video_program(c"precision mediump float;
            varying highp vec2 v_cuv; uniform sampler2D u_y; uniform sampler2D u_u; uniform sampler2D u_v;
            uniform vec3 u_scale; uniform vec3 u_coeff; uniform float u_blue; uniform float u_opacity;
            void main(){float y=(texture2D(u_y,v_cuv).r-u_scale.z)*u_scale.x;
            float u=(texture2D(u_u,v_cuv).r-128.0/255.0)*u_scale.y;
            float v=(texture2D(u_v,v_cuv).r-128.0/255.0)*u_scale.y;
            vec3 rgb=vec3(y+u_coeff.x*v,y+u_coeff.y*u+u_coeff.z*v,y+u_blue*u);
            gl_FragColor=vec4(clamp(rgb,0.0,1.0),u_opacity);}".as_ptr())
            .ok_or("Planar video shader unavailable")?;
        let mut ids = [0; 3];
        unsafe {
            glGenTextures(3, ids.as_mut_ptr());
        }
        if ids.contains(&0) {
            unsafe {
                glDeleteTextures(3, ids.as_ptr());
                glDeleteProgram(program);
            }
            return Err("Video texture allocation failed");
        }
        let locations = [
            c"u_tscreen",
            c"u_trect",
            c"u_scale",
            c"u_coeff",
            c"u_blue",
            c"u_opacity",
            c"u_y",
            c"u_u",
            c"u_v",
            c"u_uvrect",
        ]
        .map(|name| unsafe { glGetUniformLocation(program, name.as_ptr()) });
        Ok(Self {
            ids,
            program,
            context,
            context_fn,
            sizes: None,
            limits,
            format: if gl_is_es() { LUMA } else { RED },
            range: Range::Limited,
            matrix: Matrix::Bt709,
            locations,
            _main: PhantomData,
        })
    }
    fn current(&self) -> Result<(), &'static str> {
        if unsafe { (self.context_fn)() } != self.context {
            Err("Video graphics context changed")
        } else {
            Ok(())
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn upload(
        &mut self,
        _main: &MainThread,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        w: u32,
        h: u32,
        cw: u32,
        ch: u32,
        range: Range,
        matrix: Matrix,
    ) -> Result<(), &'static str> {
        self.current()?;
        validate_planes(
            y.len(),
            u.len(),
            v.len(),
            w,
            h,
            cw,
            ch,
            self.limits.max_dimension,
        )?;
        let resize = self.sizes != Some((w, h, cw, ch));
        unsafe {
            let _state = State::save();
            glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
            for (i, (bytes, pw, ph)) in [(y, w, h), (u, cw, ch), (v, cw, ch)]
                .into_iter()
                .enumerate()
            {
                glActiveTexture(GL_TEXTURE0 + i as u32);
                glBindTexture(GL_TEXTURE_2D, self.ids[i]);
                if resize {
                    tex_ledger::specified_bytes(self.ids[i], u64::from(pw) * u64::from(ph));
                    glTexImage2D(
                        GL_TEXTURE_2D,
                        0,
                        self.format as i32,
                        pw as i32,
                        ph as i32,
                        0,
                        self.format,
                        GL_UNSIGNED_BYTE,
                        std::ptr::null(),
                    );
                    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
                    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
                    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
                    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
                }
                glTexSubImage2D(
                    GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    pw as i32,
                    ph as i32,
                    self.format,
                    GL_UNSIGNED_BYTE,
                    bytes.as_ptr().cast(),
                );
            }
            if glGetError() != GL_NO_ERROR {
                self.sizes = None;
                return Err("Video texture upload failed");
            }
        }
        self.sizes = Some((w, h, cw, ch));
        self.range = range;
        self.matrix = matrix;
        Ok(())
    }
    /// Returns true when a video draw was submitted. Culled or transparent draws are skipped.
    pub fn draw(&self, main: &MainThread, rect: Rect, opacity: f32) -> Result<bool, &'static str> {
        self.draw_uv(main, rect, UV_FULL, opacity)
    }

    /// Map a source UV window onto the exact destination rectangle for all three planes.
    pub fn draw_uv(
        &self,
        _main: &MainThread,
        rect: Rect,
        uv: [f32; 4],
        opacity: f32,
    ) -> Result<bool, &'static str> {
        self.current()?;
        if self.sizes.is_none() {
            return Err("Video texture has no frame");
        }
        if ![rect.x, rect.y, rect.w, rect.h, opacity]
            .iter()
            .all(|v| v.is_finite())
            || rect.w <= 0.0
            || rect.h <= 0.0
        {
            return Err("Invalid video rectangle");
        }
        if opacity <= 0.0
            || culled(rect.x, rect.y, rect.w, rect.h)
            || gate(Class::Image, rect.x, rect.y, rect.w, rect.h)
        {
            return Ok(false);
        }
        unsafe {
            let _state = State::save();
            use_prog(self.program);
            glUniform2f(self.locations[0], SCR_W, SCR_H);
            glUniform4f(self.locations[1], rect.x, rect.y, rect.w, rect.h);
            glUniform4f(self.locations[9], uv[0], uv[1], uv[2], uv[3]);
            let (scale, coeff) = coefficients(self.range, self.matrix);
            glUniform3f(self.locations[2], scale[0], scale[1], scale[2]);
            glUniform3f(self.locations[3], coeff[0], coeff[1], coeff[2]);
            glUniform1f(
                self.locations[4],
                if self.matrix == Matrix::Bt601 {
                    1.772
                } else {
                    1.8556
                },
            );
            glUniform1f(self.locations[5], opacity.clamp(0.0, 1.0));
            for i in 0..3 {
                glActiveTexture(GL_TEXTURE0 + i as u32);
                glBindTexture(GL_TEXTURE_2D, self.ids[i]);
                glUniform1i(self.locations[6 + i], i as i32);
            }
            glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
            if glGetError() != GL_NO_ERROR {
                return Err("Video texture draw failed");
            }
        }
        Ok(true)
    }
}
impl Drop for Texture {
    fn drop(&mut self) {
        // A destroyed/replaced context already owns retirement of its GPU names.
        // Retire our logical accounting regardless; never delete a successor
        // context's coincidentally reused integer texture names.
        for id in self.ids {
            tex_ledger::deleted(id);
        }
        if self.current().is_ok() {
            unsafe {
                glDeleteTextures(3, self.ids.as_ptr());
                glDeleteProgram(self.program);
                if CUR_PROG == self.program {
                    use_prog(0);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plane_geometry_supports_odd_420_422_and_444_with_bounded_lengths() {
        assert_eq!(geometry(5, 3, 3, 2, 4096), Ok([15, 6, 6]));
        assert_eq!(geometry(5, 3, 3, 3, 4096), Ok([15, 9, 9]));
        assert_eq!(geometry(5, 3, 5, 3, 4096), Ok([15, 15, 15]));
        for dims in [(0, 1, 0, 1), (5, 3, 2, 2), (5, 3, 5, 2), (4097, 1, 2049, 1)] {
            assert!(geometry(dims.0, dims.1, dims.2, dims.3, 4096).is_err());
        }
        assert!(geometry(4096, 4096, 4096, 4096, 8192).is_err());
        assert!(geometry(640, 360, 320, 180, 512).is_err());
    }
    #[test]
    fn plane_lengths_cannot_be_short_oversized_or_interchanged() {
        assert!(validate_planes(15, 6, 6, 5, 3, 3, 2, 4096).is_ok());
        for lengths in [[14, 6, 6], [15, 5, 6], [15, 6, 7], [6, 15, 6]] {
            assert!(validate_planes(lengths[0], lengths[1], lengths[2], 5, 3, 3, 2, 4096).is_err());
        }
    }
    #[test]
    fn color_range_and_matrix_are_independent() {
        assert_ne!(
            coefficients(Range::Limited, Matrix::Bt601).0,
            coefficients(Range::Full, Matrix::Bt601).0
        );
        assert_ne!(
            coefficients(Range::Full, Matrix::Bt601).1,
            coefficients(Range::Full, Matrix::Bt709).1
        );
    }
}
