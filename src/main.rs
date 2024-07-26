#![macro_use]

fn main() {
    println!("Hello, world!");
}

use binrw::BinRead;
use binrw::BinWrite;

/// Macro which generates datapoin enum and adds extra function to retrieve the datapoint ID
///
/// # Usage
///
/// ```
/// generate_dp_enum!(
///     MyEnum {
///         Variant1(u32) = 0x10,
///         Variant2(u8) = 0x20
///     }
/// );
/// dbg!(MyEnum::Variant1(15).get_id());
/// ```
macro_rules! generate_dp_enum {
        (
            $name:ident {
                $(
                    $(#[$attr:meta])*
                    $variant:ident($type:ty) = $hex_value:expr
                ),* $(,)?
            }
        ) => {
            #[derive(Debug, Clone, PartialEq, binrw::BinRead, binrw::BinWrite)]
            #[brw(little)]
            #[br(import(id: u16))]
            pub enum $name {
                $(
                    #[br(pre_assert(id == $hex_value))]
                    $(#[$attr])*
                    $variant($type)
                ),*
            }

            impl $name {
                pub const fn get_id(&self) -> u16
                {
                    match self {
                        $(
                            $name::$variant(..) => $hex_value
                        ),*
                    }
                }
            }
        };
    }

generate_dp_enum!(
    MyEnum {
        Variant1(u32) = 0x10,
        Variant2(u8) = 0x20
    }
);

mod io {
    use std::marker::PhantomData;

    use super::MyEnum;
    use binrw::{BinRead, BinResult, BinWrite};

    pub(super) trait FromStreamPos {
        type Error;

        fn try_from_stream_position(value: u64) -> Result<Self, Self::Error>
        where
            Self: Sized;
    }

    impl FromStreamPos for () {
        type Error = ();
        fn try_from_stream_position(_: u64) -> Result<Self, Self::Error> {
            Err(())
        }
    }

    impl FromStreamPos for u8 {
        type Error = ();

        fn try_from_stream_position(value: u64) -> Result<Self, Self::Error> {
            Ok(Self::try_from(value).map_err(|_| ())?)
        }
    }

    #[binrw::binrw]
    #[brw(little)]
    pub(super) struct DpIo<SizeType, IdType>
    where
        for<'a> SizeType: FromStreamPos + BinRead<Args<'a> = ()> + BinWrite<Args<'a> = ()>,
        for<'a> IdType: BinRead<Args<'a> = ()> + BinWrite<Args<'a> = ()> + Into<u16> + Copy,
    {
        #[bw(seek_before(std::io::SeekFrom::Start(std::mem::size_of::<SizeType>() as u64)), ignore)]
        #[br(temp)]
        _size: SizeType,

        id: IdType,

        // #[bw(map_stream = DpioWriteStream::<&mut W, SizeType>::new)]
        #[br(args(id.into()))]
        #[bw(write_with = Self::enum_writer)]
        inner: MyEnum,

        /// Needed because #[br(temp)] removes SizeType usage
        marker: PhantomData<SizeType>,
    }

    pub(super) type DpIoDefault = DpIo<(), u8>;
    pub(super) type DpIoSimple = DpIo<u8, u8>;
    pub(super) type DpIoV2 = DpIo<u8, u16>;

    impl<SizeType, IdType> DpIo<SizeType, IdType>
    where
        for<'a> SizeType: FromStreamPos + BinRead<Args<'a> = ()> + BinWrite<Args<'a> = ()>,
        for<'a> IdType: Into<u16> + Copy + BinRead<Args<'a> = ()> + BinWrite<Args<'a> = ()>,
    {
        pub(super) fn inner(&self) -> MyEnum {
            self.inner.clone()
        }

        #[binrw::writer(writer, endian)]
        fn enum_writer(inner: &MyEnum) -> BinResult<()> {
            inner.write_options(writer, endian, ())?;
            let bytes_written = writer.stream_position()? - std::mem::size_of::<SizeType>() as u64;
            dbg!(bytes_written);

            if std::mem::size_of::<SizeType>() > 0 {
                let size = u8::try_from_stream_position(bytes_written).map_err(|_| {
                    binrw::Error::AssertFail {
                        pos: 0,
                        message: format!(
                            "The datapoint size of {} exceeded the maximum size of {}",
                            bytes_written,
                            u8::MAX
                        ),
                    }
                })?;

                // Reset to start and write size of datapoint
                writer.seek(std::io::SeekFrom::Start(0))?;
                size.write_options(writer, endian, ())?;
            }

            Ok(())
        }
    }

    impl DpIoDefault {
        pub(super) fn new(inner: MyEnum) -> BinResult<Self> {
            let id = inner
                .get_id()
                .try_into()
                .map_err(|_| binrw::Error::AssertFail {
                    pos: 0,
                    message: "Cannot convert value id to id format of datapoint version `Default`"
                        .to_owned(),
                })?;

            Ok(Self {
                id: id,
                inner: inner,
                marker: PhantomData,
            })
        }
    }

    impl DpIoV2 {
        pub(super) fn new(inner: MyEnum) -> Self {
            Self {
                id: inner.get_id(),
                inner,
                marker: PhantomData,
            }
        }
    }

    impl DpIoSimple {
        pub(super) fn new(inner: MyEnum) -> BinResult<Self> {
            let id = inner
                .get_id()
                .try_into()
                .map_err(|_| binrw::Error::AssertFail {
                    pos: 0,
                    message: "Cannot convert value id to id format of datapoint version `Simple`"
                        .to_owned(),
                })?;

            Ok(Self {
                id: id,
                inner: inner,
                marker: PhantomData,
            })
        }
    }
}

#[test]
fn test_enum() {
    dbg!(MyEnum::Variant1(15).get_id());
}

impl MyEnum {
    pub fn read_with_dp_version<T>(
        cursor: &mut binrw::io::Cursor<T>,
        version: u8,
    ) -> binrw::BinResult<Self>
    where
        T: AsRef<[u8]>,
    {
        use io::{DpIoDefault, DpIoSimple, DpIoV2};

        let result: Self = match version {
            0 => DpIoDefault::read(cursor)?.inner(),
            1 => DpIoSimple::read(cursor)?.inner(),
            _ => DpIoV2::read(cursor)?.inner(),
        };

        Ok(result)
    }

    pub fn write_with_dp_version<T>(
        &self,
        cursor: &mut binrw::io::Cursor<T>,
        version: u8,
    ) -> binrw::BinResult<()>
    where
        T: AsRef<[u8]>,
        std::io::Cursor<T>: std::io::Write,
    {
        use io::{DpIoDefault, DpIoSimple, DpIoV2};

        Ok(match version {
            0 => DpIoDefault::new(self.clone())?.write(cursor)?,
            1 => DpIoSimple::new(self.clone())?.write(cursor)?,
            _ => DpIoV2::new(self.clone()).write(cursor)?,
        })
    }
}

#[test]
fn test_read() {
    let mut reader = binrw::io::Cursor::new(b"\x05\x10\x01\x00\x00\x00");
    let res = MyEnum::read_with_dp_version(&mut reader, 1).unwrap();
    dbg!(res.clone());
    dbg!(res.get_id());

    let mut reader = binrw::io::Cursor::new(b"\x05\x20\x00\xFF");
    let res = MyEnum::read_with_dp_version(&mut reader, 2).unwrap();
    dbg!(res.clone());
    dbg!(res.get_id());
}

#[test]
fn test_write() -> Result<(), Box<dyn std::error::Error>> {
    use binrw::io::Read;

    let mut writer = binrw::io::Cursor::new(vec![]);
    MyEnum::Variant1(1234).write_with_dp_version(&mut writer, 0)?;
    dbg!(writer.bytes());

    let mut writer = binrw::io::Cursor::new(vec![]);
    MyEnum::Variant2(255).write_with_dp_version(&mut writer, 1)?;
    dbg!(writer.bytes());

    let mut writer = binrw::io::Cursor::new(vec![]);
    MyEnum::Variant1(1).write_with_dp_version(&mut writer, 2)?;
    dbg!(writer.bytes());

    Ok(())
}
