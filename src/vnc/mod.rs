/*
 *   This file is part of NCC Group Scrying https://github.com/nccgroup/scrying
 *   Copyright 2020 David Young <david(dot)young(at)nccgroup(dot)com>
 *   Released as open source by NCC Group Plc - https://www.nccgroup.com
 *
 *   Scrying is free software: you can redistribute it and/or modify
 *   it under the terms of the GNU General Public License as published by
 *   the Free Software Foundation, either version 3 of the License, or
 *   (at your option) any later version.
 *
 *   Scrying is distributed in the hope that it will be useful,
 *   but WITHOUT ANY WARRANTY; without even the implied warranty of
 *   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *   GNU General Public License for more details.
 *
 *   You should have received a copy of the GNU General Public License
 *   along with Scrying.  If not, see <https://www.gnu.org/licenses/>.
*/

#![allow(unused)]



use crate::argparse::Opts;
use crate::error::Error;
use crate::parsing::Target;
use crate::reporting::{AsReportMessage, ReportMessage};
use crate::util::target_to_filename;
use crate::ThreadStatus;
use image::{DynamicImage, ImageBuffer, Rgb};
#[allow(unused)]
use log::{debug, error, info, trace, warn};
use std::convert::TryInto;
use std::net::TcpStream;
use std::path::Path;
use std::sync::{mpsc, mpsc::Receiver, mpsc::Sender};
use vnc::client::{AuthChoice, AuthMethod, Client};
use vnc::{PixelFormat, Rect};

#[derive(Debug)]
pub struct VncOutput {
    target: String,
    file: String,
}

impl AsReportMessage for VncOutput {
    fn as_report_message(self) -> ReportMessage {
        ReportMessage::VncOutput(self)
    }
    fn target(&self) -> &str {
        &self.target
    }
    fn file(&self) -> &str {
        &self.file
    }
}

//TODO code reuse with RDP?
struct Image {
    image: ImageMode,
    format: PixelFormat,
    width: u16,
    height: u16,
}

impl Image {
    fn new(format: PixelFormat, width: u16, height: u16) -> Self {
        let mut image =
            ImageMode::Rgb8(DynamicImage::ImageRgb8(ImageBuffer::<
                Rgb<u8>,
                Vec<u8>,
            >::new(
                width.into(),
                height.into(),
            )));

        Self {
            image,
            format,
            width,
            height,
        }
    }

    fn put_pixels(&mut self, rect: Rect, pixels: &[u8]) -> Result<(), Error> {
        use ImageMode::*;
        trace!("pixels: {:?}", pixels);
        trace!("rect: {:?}", rect);

        //debug!("rect: {:?}", rect);
        //debug!("number of pixels: {}", pixels.len());
        //5:37:08 [DEBUG] (4) scrying::vnc: rect: Rect {
        //  left: 1216,
        //  top: 704,
        //  width: 64,
        //  height: 16
        // }
        //15:37:08 [DEBUG] (4) scrying::vnc: number of pixels: 2048
        //
        // Each pixel is made out of two items from the pixels slice

        // Borrow the pixel format from self before mutably borrowing
        // the image
        let format = &self.format;

        // Rect { left: 1216, top: 704, width: 64, height: 16 }
        let bytes_per_pixel = match format.bits_per_pixel {
            16 => 2,
            32 => 4,
            _ => {
                return Err(Error::VncError(
                    "Invalid bits per pixel".to_string(),
                ))
            }
        };
        let mut idx = 0_usize;
        for y in rect.top..(rect.top + rect.height) {
            for x in rect.left..(rect.left + rect.width) {
                trace!(
                    "Position: {},{}: {:?}",
                    x,
                    y,
                    &pixels[idx..(idx + bytes_per_pixel)]
                );

                match &mut self.image {
                    Rgb8(DynamicImage::ImageRgb8(img)) => {
                        let (r, g, b) = Image::pixel_to_rgb(
                            format,
                            &pixels[idx..(idx + bytes_per_pixel)],
                        )?;
                        img.put_pixel(x.into(), y.into(), Rgb([r, g, b]))
                    }
                    _ => unimplemented!(),
                }

                idx += bytes_per_pixel;
            }
        }

        Ok(())
    }

    /// Convert two bytes of RGB16 into their corresponding r,g,b
    /// components according to the given pixel format
    /// $ Xvfb -screen 0 800x600x24 -ac &
    /// PixelFormat {
    ///   bits_per_pixel: 16,
    ///   depth: 16,
    ///   big_endian: false,
    ///   true_colour: true,
    ///   red_max: 31,
    ///   green_max: 63,
    ///   blue_max: 31,
    ///   red_shift: 11,
    ///   green_shift: 5,
    ///   blue_shift: 0
    /// }
    ///
    /// $ Xvfb -screen 0 800x600x16 -ac &
    /// PixelFormat {
    ///   bits_per_pixel: 32,
    ///   depth: 24,
    ///   big_endian: false,
    ///   true_colour: true,
    ///   red_max: 255,
    ///   green_max: 255,
    ///   blue_max: 255,
    ///   red_shift: 16,
    ///   green_shift: 8,
    ///   blue_shift: 0
    /// }
    ///
    /// Xvfb -screen 0 800x600x15 -ac &
    /// PixelFormat {
    ///   bits_per_pixel: 16,
    ///   depth: 15,
    ///   big_endian: false,
    ///   true_colour: true,
    ///   red_max: 31,
    ///   green_max: 31,
    ///   blue_max: 31,
    ///   red_shift: 10,
    ///   green_shift: 5,
    ///   blue_shift: 0
    /// }
    ///
	/// Xvfb -screen 0 800x600x8 -ac &
	/// PixelFormat { 
	///   bits_per_pixel: 8,
	///   depth: 8, 
	///   big_endian: false,
	///   true_colour: false,
	///   red_max: 0,
	///   green_max: 0, 
	///   blue_max: 0, 
	///   red_shift: 0,
	///   green_shift: 0,
	///   blue_shift: 0 
	/// }
	/// This one results in Unsupported event: SetColourMap which we
	/// need to handle somehow

    //TODO unit test
    fn pixel_to_rgb(
        format: &PixelFormat,
        bytes: &[u8],
    ) -> Result<(u8, u8, u8), Error> {
        //TODO code reuse
        match (format.bits_per_pixel, format.depth) {
            (16, 16) | (16, 15) => {
                let bytes: [u8; 2] = bytes.try_into()?;
                let px = if format.big_endian {
                    u16::from_be_bytes(bytes)
                } else {
                    u16::from_le_bytes(bytes)
                };
                let blue_mask = format.blue_max as u16; // 5 bits
                let green_mask = format.green_max as u16; // 6 bits
                let red_mask = format.red_max as u16; // 5 bits

                let b = (px >> format.blue_shift) & blue_mask; // 0x1f
                let g = (px >> format.green_shift) & green_mask; // 0x3f
                let r = (px >> format.red_shift) & red_mask; // 0x1f

                // Left shift all the values so that they're at the top of their
                // respective bytes
                let b = b << (8 - blue_mask.count_ones()); // 3
                let g = g << (8 - green_mask.count_ones()); // 2
                let r = r << (8 - red_mask.count_ones()); // 3

                Ok((r.try_into()?, g.try_into()?, b.try_into()?))
            }
            (32, 24) => {
                let bytes: [u8; 4] = bytes.try_into()?;
                let px = if format.big_endian {
                    u32::from_be_bytes(bytes)
                } else {
                    u32::from_le_bytes(bytes)
                };
                let blue_mask = format.blue_max as u32; // 5 bits
                let green_mask = format.green_max as u32; // 6 bits
                let red_mask = format.red_max as u32; // 5 bits

                let b = (px >> format.blue_shift) & blue_mask; // 0x1f
                let g = (px >> format.green_shift) & green_mask; // 0x3f
                let r = (px >> format.red_shift) & red_mask; // 0x1f

                // Values do not need left shifting because they are
                // already 8-bits long

                Ok((r.try_into()?, g.try_into()?, b.try_into()?))
            }
            d => panic!("Unsupported colour depth {:?}", d),
        }
    }
}

enum ImageMode {
    Rgb8(DynamicImage),
}

impl ImageMode {
    fn extract(self) -> DynamicImage {
        use ImageMode::*;
        match self {
            Rgb8(di) => di,
        }
    }
}

fn vnc_capture(
    target: &Target,
    opts: &Opts,
    report_tx: &mpsc::Sender<ReportMessage>,
) -> Result<(), Error> {
    info!("Connecting to {:?}", target);
    let addr = match target {
        Target::Address(sock_addr) => sock_addr,
        Target::Url(_) => {
            return Err(Error::VncError(format!(
                "Invalid VNC target: {}",
                target
            )));
        }
    };

    let stream = TcpStream::connect(addr)?;

    let mut vnc = Client::from_tcp_stream(stream, false, |methods| {
        debug!("available auth methods: {:?}", methods);
        // Turn off Clippy's single_match check because there might be
        // other auth methods in the future
        #[allow(clippy::single_match)]
        for method in methods {
            match method {
                AuthMethod::None => return Some(AuthChoice::None),
                _ => {}
            }
        }
        warn!("AuthMethod::None may not be supported");
        None
    })?;

    let (width, height) = vnc.size();
    info!(
        "connected to \"{}\", {}x{} framebuffer",
        vnc.name(),
        width,
        height
    );

    vnc.set_encodings(&[
        vnc::Encoding::Zrle,
        vnc::Encoding::CopyRect,
        vnc::Encoding::Raw,
        vnc::Encoding::Cursor,
        vnc::Encoding::DesktopSize,
    ])?;

    let vnc_format = vnc.format();
    debug!("VNC pixel format: {:?}", vnc_format);

    debug!("requesting update");
    vnc.request_update(
        vnc::Rect {
            left: 0,
            top: 0,
            width,
            height,
        },
        false,
    )?;

    let mut vnc_image = Image::new(vnc_format, width, height);

    vnc_poll(vnc, &mut vnc_image)?;

    // Save the image
    info!("Successfully received image");
    let filename = format!("{}.png", target_to_filename(&target));
    let relative_filepath = Path::new("vnc").join(&filename);
    let filepath = Path::new(&opts.output_dir).join(&relative_filepath);
    info!("Saving image as {}", filepath.display());
    vnc_image.image.extract().save(&filepath)?;
    let vnc_message = VncOutput {
        target: target.to_string(),
        file: relative_filepath.display().to_string(),
    }
    .as_report_message();
    report_tx.send(vnc_message)?;

    Ok(())
}

fn vnc_poll(mut vnc: Client, vnc_image: &mut Image) -> Result<(), Error> {
    use vnc::client::Event::*;
    loop {
        for event in vnc.poll_iter() {
            match event {
                Disconnected(None) => {
                    warn!("VNC Channel disconnected");
                    return Ok(());
                }
                PutPixels(vnc_rect, ref pixels) => {
                    trace!("PutPixels");
                    vnc_image.put_pixels(vnc_rect, pixels)?;
                }
                EndOfFrame => {
                    debug!("End of frame");
                    return Ok(());
                }
                other => debug!("Unsupported event: {:?}", other),
            }
        }
    }
    Ok(())
}

pub fn capture(
    target: &Target,
    opts: &Opts,
    tx: mpsc::Sender<ThreadStatus>,
    report_tx: &mpsc::Sender<ReportMessage>,
) {
    if let Err(e) = vnc_capture(&target, opts, report_tx) {
        warn!("VNC error: {}", e);
    }

    tx.send(ThreadStatus::Complete).unwrap();
}