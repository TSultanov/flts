use super::*;
use crate::storage::arena::Arena;
use crate::storage::column_file::SLOT_B;
use crate::test_utils::TempDir;
use proptest::prelude::*;

#[test]
fn a_column_cannot_be_written_twice_into_one_file() {
    let dir = TempDir::new("duplicate");
    let path = dir.join("dup.dbs");

    let mut values = Arena::<u32>::new();
    values.push(1).unwrap();

    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    let err = writer.column("values", &values).unwrap_err();
    assert!(err.to_string().contains("duplicate column"), "{err}");
}

struct FailsAfter {
    written: usize,
    budget: usize,
    inner: io::Cursor<Vec<u8>>,
}

impl Write for FailsAfter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written >= self.budget {
            return Err(io::Error::other("the disk stopped taking bytes"));
        }
        self.written += buf.len();
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for FailsAfter {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.inner.seek(to)
    }
}

fn written(chunk_rows: usize, rows: u32) -> (Vec<u8>, Arena<u32>) {
    let mut values = Arena::<u32>::with_chunk_rows(chunk_rows);
    values
        .extend_from_slice(&(0..rows).collect::<Vec<u32>>())
        .unwrap();

    let mut writer =
        ColumnFileWriter::new(io::Cursor::new(Vec::new()), WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    (writer.finish().unwrap().into_inner(), values)
}

proptest! {
    #[test]
    fn the_writer_lays_every_chunk_end_to_end_after_the_head_and_never_before_it(
        chunk_rows in 1usize..6,
        rows in 0u32..40,
    ) {
        let (image, values) = written(chunk_rows, rows);
        let file_uuid = Header::decode(&image[..Header::SIZE]).file_uuid();

        let slot = CommitSlot::from_bytes(&image[SLOT_A as usize..][..CommitSlot::SIZE], file_uuid)
            .unwrap();
        let (dir_offset, dir_len, generation) = (slot.dir_offset, slot.dir_len, slot.generation);

        prop_assert_eq!(generation, 1);
        let record = &image[dir_offset as usize..];
        let directory = zstd::bulk::decompress(
            &record[9..], u32::from_le_bytes(record[4..8].try_into().unwrap()) as usize,
        ).unwrap();
        let columns = super::super::column_meta::read_directory(&directory, dir_offset).unwrap();
        let mut end = HEAD_LEN;
        for loc in &columns[0].chunks {
            prop_assert_eq!(loc.offset, end);
            prop_assert_eq!(&image[loc.offset as usize + 9..][..4], &[0x28, 0xb5, 0x2f, 0xfd]);
            end += loc.physical_len();
        }
        prop_assert_eq!(dir_offset, end);
        prop_assert_eq!(dir_offset + dir_len, image.len() as u64);
        prop_assert!(
            CommitSlot::from_bytes(&image[SLOT_B as usize..][..CommitSlot::SIZE], file_uuid)
                .is_none(),
            "a fresh file publishes through one slot only"
        );

        let mark =
            AppendMark::from_bytes(&image[APPEND_MARK as usize..][..AppendMark::SIZE], file_uuid)
                .unwrap();
        let append_from = mark.append_from;
        prop_assert_eq!(append_from, dir_offset + dir_len);

        prop_assert_eq!(values.chunk_count(), (rows as usize).div_ceil(chunk_rows));
    }

    #[test]
    fn writing_the_same_columns_twice_produces_the_same_bytes_after_the_head(
        chunk_rows in 1usize..6,
        rows in 0u32..40,
    ) {
        let (once, _) = written(chunk_rows, rows);
        let (twice, _) = written(chunk_rows, rows);

        let at = HEAD_LEN as usize;
        prop_assert_eq!(&once[at..], &twice[at..]);
        prop_assert_ne!(&once[..Header::SIZE], &twice[..Header::SIZE]);
    }

    #[test]
    fn a_writer_that_cannot_write_reports_it_instead_of_lying(budget in 0usize..16_550) {
        let mut values = Arena::<u32>::with_chunk_rows(4);
        values.extend_from_slice(&(0..40).collect::<Vec<u32>>()).unwrap();

        let out = FailsAfter {
            written: 0,
            budget,
            inner: io::Cursor::new(Vec::new()),
        };

        let failed = ColumnFileWriter::new(out, WriterId::ANONYMOUS)
            .and_then(|mut writer| {
                writer.column("values", &values)?;
                writer.finish()
            })
            .is_err();

        prop_assert!(failed, "a writer that ran out of disk at {budget} bytes reported success");
    }
}
