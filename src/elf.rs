//! Minimal ELF64 loader support for Oxenna userspace.
//!
//! `.ox` files are ordinary x86_64 ELF64 binaries.  The extension is only a
//! shell/filesystem convention; the bytes on disk are still ELF.

#![allow(dead_code)]

pub const ELF_MAGIC: [u8; 4] = *b"\x7fELF";
pub const PT_LOAD: u32 = 1;
pub const PT_INTERP: u32 = 3;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_X86_64: u16 = 62;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    WrongMachine,
    UnsupportedType,
    BadHeader,
    BadProgramHeader,
    Interpreter,
    AddressOverflow,
    InvalidSegment,
}

#[derive(Clone, Copy)]
pub struct Elf64 {
    pub entry: u64,
    pub phoff: u64,
    pub phentsize: u16,
    pub phnum: u16,
    pub kind: u16,
}

#[derive(Clone, Copy)]
pub struct ProgramHeader {
    pub typ: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o], b[o + 1], b[o + 2], b[o + 3],
        b[o + 4], b[o + 5], b[o + 6], b[o + 7],
    ])
}

impl Elf64 {
    pub fn parse(image: &[u8]) -> Result<Self, ElfError> {
        if image.len() < 64 {
            return Err(ElfError::TooSmall);
        }
        if image[0..4] != ELF_MAGIC {
            return Err(ElfError::BadMagic);
        }
        if image[4] != 2 {
            return Err(ElfError::Not64Bit);
        }
        if image[5] != 1 {
            return Err(ElfError::NotLittleEndian);
        }
        if image[6] != 1 {
            return Err(ElfError::BadHeader);
        }

        let kind = u16le(image, 16);
        let supported_type =
            (kind == ET_DYN && cfg!(feature = "elf_et_dyn")) ||
            (kind == ET_EXEC && cfg!(feature = "elf_et_exec"));
        if !supported_type {
            return Err(ElfError::UnsupportedType);
        }
        if u16le(image, 18) != EM_X86_64 {
            return Err(ElfError::WrongMachine);
        }

        let phoff = u64le(image, 32);
        let phentsize = u16le(image, 54);
        let phnum = u16le(image, 56);
        if phentsize < 56 {
            return Err(ElfError::BadHeader);
        }

        let ph_end = phoff
            .checked_add((phentsize as u64).checked_mul(phnum as u64).ok_or(ElfError::BadHeader)?)
            .ok_or(ElfError::BadHeader)?;
        if ph_end > image.len() as u64 {
            return Err(ElfError::BadHeader);
        }

        Ok(Self {
            entry: u64le(image, 24),
            phoff,
            phentsize,
            phnum,
            kind,
        })
    }

    pub fn program_header(&self, image: &[u8], index: u16) -> Result<ProgramHeader, ElfError> {
        if index >= self.phnum {
            return Err(ElfError::BadProgramHeader);
        }
        let o = self.phoff as usize + index as usize * self.phentsize as usize;
        if o.checked_add(56).map_or(true, |e| e > image.len()) {
            return Err(ElfError::BadProgramHeader);
        }

        Ok(ProgramHeader {
            typ: u32le(image, o),
            flags: u32le(image, o + 4),
            offset: u64le(image, o + 8),
            vaddr: u64le(image, o + 16),
            filesz: u64le(image, o + 32),
            memsz: u64le(image, o + 40),
            align: u64le(image, o + 48),
        })
    }

    pub fn load_headers<'a>(
        &'a self,
        image: &'a [u8],
    ) -> Result<alloc::vec::Vec<ProgramHeader>, ElfError> {
        let mut out = alloc::vec::Vec::new();
        for i in 0..self.phnum {
            let ph = self.program_header(image, i)?;
            if ph.typ == PT_INTERP {
                return Err(ElfError::Interpreter);
            }
            if ph.typ == PT_LOAD {
                if ph.filesz > ph.memsz {
                    return Err(ElfError::InvalidSegment);
                }
                if ph.offset.checked_add(ph.filesz).map_or(true, |e| e > image.len() as u64) {
                    return Err(ElfError::InvalidSegment);
                }
                if ph.vaddr.checked_add(ph.memsz).is_none() {
                    return Err(ElfError::AddressOverflow);
                }
                if ph.memsz != 0 {
                    out.push(ph);
                }
            }
        }
        if out.is_empty() {
            return Err(ElfError::InvalidSegment);
        }
        Ok(out)
    }
}
