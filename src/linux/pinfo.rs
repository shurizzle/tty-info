use core::{fmt, mem::MaybeUninit};

use crate::{CStr, Dev, DirentBuf, Errno, RawFd, TtyInfo};
use atoi::FromRadix10Signed;
use linux_syscalls::{syscall, Sysno};

use super::{DirBuf, PathBuf};

const SELF_INFO_PATH: &[u8] = b"/proc/self/stat\0".as_slice();
const O_CLOEXEC: usize = 0o2000000;
const O_RDONLY: usize = 0;

unsafe fn parse_num<T: FromRadix10Signed>(buf: &[u8]) -> Result<(T, &[u8]), Errno> {
    let (res, len) = T::from_radix_10_signed(buf);
    if len == 0 {
        return Err(Errno::EINVAL);
    }
    let buf = buf.get_unchecked(len..);
    Ok((res, buf))
}

unsafe fn skip_char(buf: &[u8], ch: u8) -> Result<&[u8], Errno> {
    if buf.iter().copied().next().map_or(false, |c| c == ch) {
        Ok(buf.get_unchecked(1..))
    } else {
        Err(Errno::EINVAL)
    }
}

#[inline]
unsafe fn skip_space(buf: &[u8]) -> Result<&[u8], Errno> {
    skip_char(buf, b' ')
}

#[derive(Debug, Clone, Copy, Hash)]
pub struct RawProcessInfo {
    pub pid: u32,
    pub session: u32,
    pub tty_nr: Option<Dev>,
}

impl RawProcessInfo {
    fn parse(path: &CStr) -> Result<Self, Errno> {
        unsafe {
            struct FdHolder(RawFd);
            impl Drop for FdHolder {
                fn drop(&mut self) {
                    _ = unsafe { syscall!([ro] Sysno::close, self.0) };
                }
            }

            let mut buf = MaybeUninit::<[u8; 1024]>::uninit();
            let mut len: usize = 0;
            {
                let fd = syscall!([ro] Sysno::open, path.as_ptr(), O_RDONLY | O_CLOEXEC)? as RawFd;
                let _h = FdHolder(fd);
                let mut b = core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), 1024);
                while !b.is_empty() {
                    match syscall!(Sysno::read, fd, b.as_mut_ptr(), b.len()) {
                        Ok(0) => break,
                        Ok(n) => {
                            len += n;
                            b = b.get_unchecked_mut(n..);
                        }
                        Err(Errno::EAGAIN) | Err(Errno::EINTR) => (),
                        Err(err) => return Err(err),
                    }
                }
            }
            let buf = buf.assume_init();
            let buf = buf.get_unchecked(..len);

            let (pid, buf) = parse_num(buf)?;
            let buf = skip_space(buf)?;

            let buf = skip_char(buf, b'(')?;
            let buf = match memchr::memchr(b')', buf) {
                Some(i) => skip_space(buf.get_unchecked((i + 1)..))?,
                None => return Err(Errno::EINVAL),
            };

            let buf = match memchr::memchr(b' ', buf) {
                Some(1) => buf.get_unchecked(2..),
                Some(_) | None => return Err(Errno::EINVAL),
            };

            let (_, buf) = parse_num::<core::ffi::c_int>(buf)?;
            let buf = skip_space(buf)?;
            let buf = match memchr::memchr(b' ', buf) {
                Some(0) | None => return Err(Errno::EINVAL),
                Some(n) => buf.get_unchecked((n + 1)..),
            };
            let (session, buf) = parse_num(buf)?;
            let buf = skip_space(buf)?;
            let tty_nr = parse_num::<i32>(buf)?.0;
            let tty_nr = if tty_nr == -1 {
                None
            } else {
                Some(core::mem::transmute::<i32, u32>(tty_nr).into())
            };

            Ok(Self {
                pid,
                session,
                tty_nr,
            })
        }
    }

    #[inline]
    pub fn current() -> Result<Self, Errno> {
        Self::parse(unsafe { CStr::from_ptr(SELF_INFO_PATH.as_ptr().cast()) })
    }

    pub fn for_process(pid: u32) -> Result<Self, Errno> {
        use itoap::Integer;

        let mut uninit_buf = MaybeUninit::<[u8; 11 + core::ffi::c_int::MAX_LEN + 1]>::uninit();
        let path = unsafe {
            let mut buf = uninit_buf.as_mut_ptr().cast::<u8>();
            core::ptr::copy_nonoverlapping(b"/proc/".as_ptr().cast::<u8>(), buf, 6);
            buf = buf.add(6);
            let len = itoap::write_to_ptr(buf, pid);
            buf = buf.add(len);
            core::ptr::copy_nonoverlapping(b"/stat".as_ptr().cast::<u8>(), buf, 5);
            *buf.add(5) = 0;
            CStr::from_ptr((uninit_buf.as_mut_ptr().cast::<u8>() as *const u8).cast())
        };

        Self::parse(path)
    }
}

#[derive(Clone)]
pub struct ProcessInfo<B: DirentBuf> {
    pub pid: u32,
    pub session: u32,
    pub tty: Option<TtyInfo<B>>,
}

impl<B: DirentBuf> fmt::Debug for ProcessInfo<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessInfo")
            .field("pid", &self.pid)
            .field("session", &self.session)
            .field("tty", &self.tty)
            .finish()
    }
}

impl<B: DirentBuf> ProcessInfo<B> {
    pub fn current_with_buffers_in<'a, I, B1>(dirs: I, buf: &mut B1, path: B) -> Result<Self, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
        B1: DirentBuf,
    {
        let raw = RawProcessInfo::current()?;

        Ok(Self {
            pid: raw.pid,
            session: raw.session,
            tty: if let Some(tty_nr) = raw.tty_nr {
                Some(TtyInfo::by_number_with_buffers_in(tty_nr, dirs, buf, path)?)
            } else {
                None
            },
        })
    }

    pub fn for_process_with_buffers_in<'a, I, B1>(
        pid: u32,
        dirs: I,
        buf: &mut B1,
        path: B,
    ) -> Result<Self, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
        B1: DirentBuf,
    {
        let raw = RawProcessInfo::for_process(pid)?;

        Ok(Self {
            pid: raw.pid,
            session: raw.session,
            tty: if let Some(tty_nr) = raw.tty_nr {
                Some(TtyInfo::by_number_with_buffers_in(tty_nr, dirs, buf, path)?)
            } else {
                None
            },
        })
    }

    pub fn current_with_buffers<B1>(buf: &mut B1, path: B) -> Result<Self, Errno>
    where
        B1: DirentBuf,
    {
        Self::current_with_buffers_in(
            [unsafe { CStr::from_bytes_with_nul_unchecked(b"/dev\0") }],
            buf,
            path,
        )
    }

    pub fn for_process_with_buffers<B1>(pid: u32, buf: &mut B1, path: B) -> Result<Self, Errno>
    where
        B1: DirentBuf,
    {
        Self::for_process_with_buffers_in(
            pid,
            [unsafe { CStr::from_bytes_with_nul_unchecked(b"/dev\0") }],
            buf,
            path,
        )
    }
}

impl ProcessInfo<PathBuf> {
    #[inline]
    pub fn current_in<'a, I>(dirs: I) -> Result<Self, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
    {
        Self::current_with_buffers_in(dirs, &mut DirBuf::new(), PathBuf::new())
    }

    #[inline]
    pub fn for_process_in<'a, I>(pid: u32, dirs: I) -> Result<Self, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
    {
        Self::for_process_with_buffers_in(pid, dirs, &mut DirBuf::new(), PathBuf::new())
    }

    #[inline]
    pub fn current() -> Result<Self, Errno> {
        Self::current_with_buffers(&mut DirBuf::new(), PathBuf::new())
    }

    #[inline]
    pub fn for_process(pid: u32) -> Result<Self, Errno> {
        Self::for_process_with_buffers(pid, &mut DirBuf::new(), PathBuf::new())
    }
}
