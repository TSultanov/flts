//! A second implementation of the on-disk format, for files the writer would refuse to write.

pub const HEAD_LEN: u64 = 16384;
pub const SLOT_A: u64 = 4096;
pub const SLOT_B: u64 = 8192;
pub const APPEND_MARK: u64 = 12288;
pub const MAGIC: [u8; 4] = *b"DBS0";
pub const SLOT_MAGIC: [u8; 4] = *b"DBSC";
pub const MARK_MAGIC: [u8; 4] = *b"DBSA";
pub const VERSION: u16 = 1;

const OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const PRIME: u64 = 1_099_511_628_211;

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[derive(Clone, Debug)]
pub struct Chunk {
    pub rows: u32,
    pub bytes: Vec<u8>,
    pub offset: Option<u64>,
    pub byte_len: Option<u32>,
    pub fnv: Option<u64>,
}

impl Chunk {
    pub fn of(rows: u32, elem_size: u32) -> Self {
        let byte_len = (rows * elem_size) as usize;
        Self {
            rows,
            bytes: (0..byte_len).map(|at| at as u8).collect(),
            offset: None,
            byte_len: None,
            fnv: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub elem_size: u32,
    pub chunk_rows: u64,
    pub rows: Option<u64>,
    pub codec: u8,
    pub chunks: Vec<Chunk>,
}

impl Column {
    pub fn of(name: &str, elem_size: u32, chunk_rows: u64, chunk_counts: &[u32]) -> Self {
        Self {
            name: name.to_owned(),
            elem_size,
            chunk_rows,
            rows: None,
            codec: 0,
            chunks: chunk_counts
                .iter()
                .map(|rows| Chunk::of(*rows, elem_size))
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileModel {
    pub file_uuid: u128,
    pub version: u16,
    pub magic: [u8; 4],
    pub generation: u64,
    pub writer_id: u64,
    pub columns: Vec<Column>,
    pub column_count: Option<u32>,
    pub dir_offset: Option<u64>,
    pub dir_len: Option<u64>,
    pub append_from: Option<u64>,
    pub second_slot: bool,
}

impl FileModel {
    pub fn of(columns: Vec<Column>) -> Self {
        Self {
            file_uuid: 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef,
            version: VERSION,
            magic: MAGIC,
            generation: 1,
            writer_id: 0,
            columns,
            column_count: None,
            dir_offset: None,
            dir_len: None,
            append_from: None,
            second_slot: false,
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut image = vec![0u8; HEAD_LEN as usize];
        self.write_header(&mut image);

        let mut placed = self.columns.clone();
        for column in &mut placed {
            for chunk in &mut column.chunks {
                if chunk.offset.is_none() {
                    chunk.offset = Some(image.len() as u64);
                }
                image.extend_from_slice(&chunk.bytes);
            }
        }

        let directory = write_directory(&placed, self.column_count);
        let dir_offset = self.dir_offset.unwrap_or(image.len() as u64);
        let dir_len = self.dir_len.unwrap_or(directory.len() as u64);
        if self.dir_offset.is_none() {
            image.extend_from_slice(&directory);
        }

        let slot_at = if self.second_slot { SLOT_B } else { SLOT_A };
        let slot = slot_bytes(
            self.generation,
            self.file_uuid,
            self.writer_id,
            dir_offset,
            dir_len,
            fnv1a(&directory),
        );
        image[slot_at as usize..slot_at as usize + slot.len()].copy_from_slice(&slot);

        let mark = mark_bytes(
            self.file_uuid,
            self.append_from.unwrap_or(dir_offset + dir_len),
            self.generation,
        );
        let at = APPEND_MARK as usize;
        image[at..at + mark.len()].copy_from_slice(&mark);

        image
    }

    fn write_header(&self, image: &mut [u8]) {
        image[..4].copy_from_slice(&self.magic);
        image[4..6].copy_from_slice(&self.version.to_le_bytes());
        image[6..8].copy_from_slice(&0u16.to_le_bytes());
        image[8..24].copy_from_slice(&self.file_uuid.to_le_bytes());
    }
}

fn write_directory(columns: &[Column], count: Option<u32>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&count.unwrap_or(columns.len() as u32).to_le_bytes());

    for column in columns {
        out.extend_from_slice(&(column.name.len() as u32).to_le_bytes());
        out.extend_from_slice(column.name.as_bytes());
        out.extend_from_slice(&column.elem_size.to_le_bytes());
        out.extend_from_slice(&column.chunk_rows.to_le_bytes());
        let rows = column.rows.unwrap_or_else(|| {
            column
                .chunks
                .iter()
                .map(|chunk| u64::from(chunk.rows))
                .sum()
        });
        out.extend_from_slice(&rows.to_le_bytes());
        out.push(column.codec);
        out.extend_from_slice(&(column.chunks.len() as u32).to_le_bytes());

        for chunk in &column.chunks {
            out.extend_from_slice(&chunk.offset.unwrap_or(HEAD_LEN).to_le_bytes());
            let byte_len = chunk.byte_len.unwrap_or(chunk.bytes.len() as u32);
            out.extend_from_slice(&byte_len.to_le_bytes());
            out.extend_from_slice(&chunk.rows.to_le_bytes());
            out.extend_from_slice(
                &chunk
                    .fnv
                    .unwrap_or_else(|| fnv1a(&chunk.bytes))
                    .to_le_bytes(),
            );
        }
    }

    out
}

fn slot_bytes(
    generation: u64,
    file_uuid: u128,
    writer_id: u64,
    dir_offset: u64,
    dir_len: u64,
    dir_fnv: u64,
) -> [u8; 80] {
    let mut out = [0u8; 80];
    out[..4].copy_from_slice(&SLOT_MAGIC);
    out[8..16].copy_from_slice(&generation.to_le_bytes());
    out[16..24].copy_from_slice(&0u64.to_le_bytes());
    out[24..40].copy_from_slice(&file_uuid.to_le_bytes());
    out[40..48].copy_from_slice(&writer_id.to_le_bytes());
    out[48..56].copy_from_slice(&dir_offset.to_le_bytes());
    out[56..64].copy_from_slice(&dir_len.to_le_bytes());
    out[64..72].copy_from_slice(&dir_fnv.to_le_bytes());
    let signature = fnv1a(&out[..72]);
    out[72..].copy_from_slice(&signature.to_le_bytes());
    out
}

fn mark_bytes(file_uuid: u128, append_from: u64, generation: u64) -> [u8; 48] {
    let mut out = [0u8; 48];
    out[..4].copy_from_slice(&MARK_MAGIC);
    out[8..24].copy_from_slice(&file_uuid.to_le_bytes());
    out[24..32].copy_from_slice(&append_from.to_le_bytes());
    out[32..40].copy_from_slice(&generation.to_le_bytes());
    let signature = fnv1a(&out[..40]);
    out[40..].copy_from_slice(&signature.to_le_bytes());
    out
}
