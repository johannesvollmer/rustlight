use crate::constants;
use crate::geometry::Mesh;
use crate::math::Frame;
use crate::tools::*;
use crate::Scale;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use cgmath::{EuclideanSpace, Point2, Point3, Vector2, Vector3};
#[cfg(feature = "image")]
use image::{DynamicImage, GenericImage, Pixel};
#[cfg(feature = "exr")]
use exr;
use std;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::ops::*;
use std::path::Path;

#[derive(Clone, Debug)]
pub enum PDF {
    SolidAngle(f32),
    Area(f32),
    Discrete(f32),
}

#[derive(PartialEq, Clone, Copy)]
pub enum Domain {
    SolidAngle,
    Discrete,
}

impl PDF {
    pub fn is_zero(&self) -> bool {
        match self {
            PDF::Discrete(v) | PDF::SolidAngle(v) | PDF::Area(v) => (*v == 0.0),
        }
    }

    pub fn value(&self) -> f32 {
        match self {
            PDF::Discrete(v) | PDF::SolidAngle(v) | PDF::Area(v) => *v,
        }
    }
}

impl Mul<f32> for PDF {
    type Output = PDF;
    fn mul(self, other: f32) -> PDF {
        match self {
            PDF::Area(v) => PDF::Area(v * other),
            PDF::Discrete(v) => PDF::Discrete(v * other),
            PDF::SolidAngle(v) => PDF::SolidAngle(v * other),
        }
    }
}

pub struct SampledPosition {
    pub p: Point3<f32>,
    pub n: Vector3<f32>,
    pub pdf: PDF,
}

/// Pixel color representation
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Copy)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Color {
    pub fn new(r: f32, g: f32, b: f32) -> Color {
        Color { r, g, b }
    }
    pub fn zero() -> Color {
        Color::new(0.0, 0.0, 0.0)
    }
    pub fn one() -> Color {
        Color::new(1.0, 1.0, 1.0)
    }
    pub fn value(v: f32) -> Color {
        Color::new(v, v, v)
    }
    pub fn abs(&self) -> Color {
        Color::new(self.r.abs(), self.g.abs(), self.b.abs())
    }
    pub fn sqrt(self) -> Color {
        Color::new(self.r.sqrt(), self.g.sqrt(), self.b.sqrt())
    }
    pub fn avg(&self) -> f32 {
        (self.r + self.g + self.b) / 3.0
    }
    pub fn exp(self) -> Color {
        Color::new(self.r.exp(), self.g.exp(), self.b.exp())
    }
    pub fn get(&self, c: u8) -> f32 {
        match c {
            0 => self.r,
            1 => self.g,
            2 => self.b,
            _ => unimplemented!("Impossible to have more than 3 channels"),
        }
    }

    pub fn is_zero(&self) -> bool {
        self.r == 0.0 && self.g == 0.0 && self.b == 0.0
    }

    #[cfg(feature = "image")]
    pub fn to_rgba(&self) -> image::Rgba<u8> {
        image::Rgba::from_channels(
            (self.r.min(1.0).powf(1.0 / 2.2) * 255.0) as u8,
            (self.g.min(1.0).powf(1.0 / 2.2) * 255.0) as u8,
            (self.b.min(1.0).powf(1.0 / 2.2) * 255.0) as u8,
            255,
        )
    }
    pub fn channel_max(&self) -> f32 {
        self.r.max(self.g.max(self.b))
    }

    pub fn luminance(&self) -> f32 {
        // FIXME: sRGB??
        self.r * 0.212_671 + self.g * 0.715_160 + self.b * 0.072_169
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::zero()
    }
}

impl Scale<f32> for Color {
    fn scale(&mut self, v: f32) {
        self.r *= v;
        self.g *= v;
        self.b *= v;
    }
}

impl Neg for Color {
    type Output = Color;
    fn neg(self) -> Self::Output {
        Color::new(-self.r, -self.g, -self.b)
    }
}

/////////////// Operators
impl DivAssign<f32> for Color {
    fn div_assign(&mut self, other: f32) {
        self.r /= other;
        self.g /= other;
        self.b /= other;
    }
}

impl<'b> MulAssign<Color> for Color {
    fn mul_assign(&mut self, other: Color) {
        self.r *= other.r;
        self.g *= other.g;
        self.b *= other.b;
    }
}

impl<'b> MulAssign<&'b Color> for Color {
    fn mul_assign(&mut self, other: &'b Color) {
        self.r *= other.r;
        self.g *= other.g;
        self.b *= other.b;
    }
}

impl MulAssign<f32> for Color {
    fn mul_assign(&mut self, other: f32) {
        self.r *= other;
        self.g *= other;
        self.b *= other;
    }
}

impl<'b> AddAssign<&'b Color> for Color {
    fn add_assign(&mut self, other: &'b Color) {
        self.r += other.r;
        self.g += other.g;
        self.b += other.b;
    }
}

impl AddAssign<Color> for Color {
    fn add_assign(&mut self, other: Color) {
        self.r += other.r;
        self.g += other.g;
        self.b += other.b;
    }
}

impl Div<f32> for Color {
    type Output = Self;
    fn div(self, other: f32) -> Color {
        assert!(other.is_finite());
        assert_ne!(other, 0.0);
        Color {
            r: self.r / other,
            g: self.g / other,
            b: self.b / other,
        }
    }
}

impl Div<Color> for Color {
    type Output = Self;
    fn div(self, other: Color) -> Color {
        Color {
            r: self.r / other.r,
            g: self.g / other.g,
            b: self.b / other.b,
        }
    }
}

impl Mul<f32> for Color {
    type Output = Self;
    fn mul(self, other: f32) -> Color {
        //assert!(other.is_finite());
        if other.is_finite() {
            Color {
                r: self.r * other,
                g: self.g * other,
                b: self.b * other,
            }
        } else {
            Color::zero()
        }
    }
}

impl Mul<Color> for f32 {
    type Output = Color;
    fn mul(self, other: Color) -> Color {
        Color {
            r: other.r * self,
            g: other.g * self,
            b: other.b * self,
        }
    }
}

impl<'a, 'b> Sub<&'a Color> for &'b Color {
    type Output = Color;
    fn sub(self, other: &'a Color) -> Color {
        Color {
            r: other.r - self.r,
            g: other.g - self.g,
            b: other.b - self.b,
        }
    }
}

impl<'a> Mul<&'a Color> for f32 {
    type Output = Color;
    fn mul(self, other: &'a Color) -> Color {
        Color {
            r: other.r * self,
            g: other.g * self,
            b: other.b * self,
        }
    }
}

impl<'a> Mul<&'a Color> for Color {
    type Output = Self;
    fn mul(self, other: &'a Color) -> Color {
        Color {
            r: self.r * other.r,
            g: self.g * other.g,
            b: self.b * other.b,
        }
    }
}

impl Mul<Color> for Color {
    type Output = Self;
    fn mul(self, other: Color) -> Color {
        Color {
            r: self.r * other.r,
            g: self.g * other.g,
            b: self.b * other.b,
        }
    }
}

impl Sub<Color> for Color {
    type Output = Self;
    fn sub(self, other: Color) -> Color {
        Color {
            r: self.r - other.r,
            g: self.g - other.g,
            b: self.b - other.b,
        }
    }
}

impl Add<Color> for Color {
    type Output = Self;
    fn add(self, other: Color) -> Color {
        Color {
            r: self.r + other.r,
            g: self.g + other.g,
            b: self.b + other.b,
        }
    }
}

impl<'a> Add<&'a Color> for Color {
    type Output = Self;
    fn add(self, other: &'a Color) -> Color {
        Color {
            r: self.r + other.r,
            g: self.g + other.g,
            b: self.b + other.b,
        }
    }
}

pub struct Bitmap {
    pub size: Vector2<u32>,
    pub colors: Vec<Color>,
}
impl Bitmap {
    pub fn new(size: Vector2<u32>) -> Bitmap {
        Bitmap {
            size,
            colors: vec![Color::default(); (size.x * size.y) as usize],
        }
    }
    pub fn clear(&mut self) {
        self.colors.iter_mut().for_each(|x| *x = Color::default());
    }
    pub fn accumulate(&mut self, p: Point2<u32>, f: Color) {
        assert!(p.x < self.size.x);
        assert!(p.y < self.size.y);
        let index = (p.y * self.size.x + p.x) as usize;
        self.colors[index] += f;
    }
    /**
     * pos: Position where to splat the buffer
     */
    pub fn accumulate_bitmap(&mut self, o: &Bitmap, pos: Point2<u32>) {
        for y in 0..o.size.y {
            for x in 0..o.size.x {
                let p = Point2::new(pos.x + x, pos.y + y);
                let index = (p.y * self.size.x + p.x) as usize;
                let index_other = (y * o.size.x + x) as usize;
                self.colors[index] += o.colors[index_other];
            }
        }
    }
    pub fn scale(&mut self, v: f32) {
        self.colors.iter_mut().for_each(|x| x.scale(v));
    }
    pub fn average(&self) -> Color {
        let mut s = Color::default();
        self.colors.iter().for_each(|x| s += x);
        s.scale(1.0 / self.colors.len() as f32);
        s
    }

    // Get the pixel value at the given position
    pub fn pixel_uv(&self, mut uv: Vector2<f32>) -> Color {
        uv.x = uv.x.modulo(1.0);
        uv.y = uv.y.modulo(1.0);
        let (x, y) = (
            (uv.x * self.size.x as f32) as usize,
            (uv.y * self.size.y as f32) as usize,
        );
        let i = self.size.x as usize * y + x;
        if i >= self.colors.len() {
            warn!(
                "Exceed UV coordinates: {:?} | {:?} | {:?}",
                uv,
                self.size,
                (x, y)
            );
            Color::default()
        } else {
            self.colors[i]
        }
    }
    pub fn pixel(&self, p: Point2<u32>) -> Color {
        assert!(p.x < self.size.x);
        assert!(p.y < self.size.y);
        self.colors[(p.y * self.size.x + p.x) as usize]
    }

    // Save functions
    #[cfg(not(feature = "image"))]
    pub fn save_ldr_image(&self, _imgout_path_str: &str) {
        panic!("Rustlight wasn't built with Image support.");
    }
    #[cfg(feature = "image")]
    pub fn save_ldr_image(&self, imgout_path_str: &str) {
        // The image that we will render
        let mut image_ldr = DynamicImage::new_rgb8(self.size.x, self.size.y);
        for x in 0..self.size.x {
            for y in 0..self.size.y {
                let p = Point2::new(x, y);
                image_ldr.put_pixel(x, y, self.pixel(p).to_rgba())
            }
        }
        image_ldr
            .save(&Path::new(imgout_path_str))
            .expect("failed to write img into file");
    }

    #[cfg(not(feature = "exr"))]
    pub fn save_exr(&self, _imgout_path_str: &str) {
        panic!("Rustlight wasn't built with OpenExr support.");
    }
    #[cfg(feature = "exr")]
    pub fn save_exr(&self, imgout_path_str: &str) {

        // Pixel data for floating point RGB image.
        let mut pixel_data = Vec::with_capacity((self.size.x * self.size.y * 3) as usize);

        // exrs TODO write without pre-allocating a buffer!
        for y in 0..self.size.y {
            for x in 0..self.size.x {
                let rgb = self.pixel(Point2::new(x, y));
                pixel_data.extend_from_slice(&[rgb.r, rgb.g, rgb.b]);
            }
        }

        // Create a file to write to.
        use exr::prelude::*;

        let output_file = rgba::Image::new(
            Vec2(self.size.x as usize, self.size.y as usize),
            false, true,
            rgba::Pixels::F32(pixel_data)
        );

        // Write pixel data to the file.
        output_file.write_to_file(imgout_path_str, write_options::default()).unwrap();
    }
    pub fn save(&self, imgout_path_str: &str) {
        let output_ext = match std::path::Path::new(imgout_path_str).extension() {
            None => panic!("No file extension provided"),
            Some(x) => std::ffi::OsStr::to_str(x).expect("Issue to unpack the file"),
        };
        match output_ext {
            "pfm" => {
                self.save_pfm(imgout_path_str);
            }
            "png" => {
                self.save_ldr_image(imgout_path_str);
            }
            "exr" => {
                self.save_exr(imgout_path_str);
            }
            _ => panic!("Unknown output file extension"),
        }
    }

    pub fn save_pfm(&self, imgout_path_str: &str) {
        let file = File::create(Path::new(imgout_path_str)).unwrap();
        let mut file = BufWriter::new(file);
        let header = format!("PF\n{} {}\n-1.0\n", self.size.x, self.size.y);
        file.write_all(header.as_bytes()).unwrap();
        for y in 0..self.size.y {
            for x in 0..self.size.x {
                let p = self.pixel(Point2::new(x, self.size.y - y - 1));
                file.write_f32::<LittleEndian>(p.r.abs()).unwrap();
                file.write_f32::<LittleEndian>(p.g.abs()).unwrap();
                file.write_f32::<LittleEndian>(p.b.abs()).unwrap();
            }
        }
    }

    // Load images
    pub fn read_pfm(filename: &str) -> Self {
        let f = File::open(Path::new(filename)).unwrap();
        let mut f = BufReader::new(f);
        // Check the flag
        {
            let mut header_str = String::new();
            f.read_line(&mut header_str).unwrap();
            if header_str != "PF\n" {
                panic!("Wrong PF flag encounter");
            }
        }
        // Check the dim
        let size = {
            let mut img_dim_y = String::new();
            f.read_line(&mut img_dim_y).unwrap();
            let mut img_dim_x = String::new();
            f.read_line(&mut img_dim_x).unwrap();
            Vector2::new(
                img_dim_x.parse::<u32>().unwrap(),
                img_dim_y.parse::<u32>().unwrap(),
            )
        };

        let mut colors = vec![Color::zero(); (size.x * size.y) as usize];
        for y in 0..size.y {
            for x in 0..size.x {
                let r = f.read_f32::<LittleEndian>().unwrap();
                let g = f.read_f32::<LittleEndian>().unwrap();
                let b = f.read_f32::<LittleEndian>().unwrap();
                //
                let p = Point2::new(x, size.y - y - 1);
                colors[(p.y * size.x + p.x) as usize] = Color::new(r, g, b);
            }
        }

        Bitmap { size, colors }
    }

    #[cfg(not(feature = "exr"))]
    pub fn read_exr(_filename: &str) -> Self {
        panic!("Rustlight wasn't built with OpenEXR support");
    }

    #[cfg(feature = "exr")]
    pub fn read_exr(filename: &str) -> Self {
        use exr::prelude::*;

        // Open the EXR file.
        // exrs TODO does not support all compression types
        // exrs TODO write without pre-allocating a buffer but instead take a closure that convers the stuff
        let image = rgba::Image::read_from_file(filename, read_options::default()).unwrap();

        debug_assert!(!image.has_alpha_channel); // exrs TODO add simple pixel iterator!

        match image.data {
            rgba::Pixels::F32(data) => {
                let colors = image.data.into_iter()
                    .windows(3)
                    .map(|color| Color::new(v[0], v[1], v[2]))
                    .collect();

                Bitmap { size, colors }
            },

            _ => unimplemented!("only f32 data for now")
        }
    }

    #[cfg(not(feature = "image"))]
    pub fn read_ldr_image(_filename: &str) -> Self {
        panic!("Rustlight wasn't built with image support");
        Bitmap::default()
    }
    #[cfg(feature = "image")]
    pub fn read_ldr_image(filename: &str) -> Self {
        // The image that we will render
        let image_ldr = image::open(filename)
            .unwrap_or_else(|_| panic!("Impossible to read image: {}", filename));
        let image_ldr = image_ldr.to_rgb();
        let size = Vector2::new(image_ldr.width(), image_ldr.height());
        let mut colors = vec![Color::zero(); (size.x * size.y) as usize];
        for x in 0..size.x {
            for y in 0..size.y {
                let p = image_ldr.get_pixel(x, y);
                colors[(y * size.x + x) as usize] = Color::new(
                    f32::from(p[0]) / 255.0,
                    f32::from(p[1]) / 255.0,
                    f32::from(p[2]) / 255.0,
                );
            }
        }

        Bitmap { size, colors }
    }

    pub fn read(filename: &str) -> Self {
        let ext = match std::path::Path::new(filename).extension() {
            None => panic!("No file extension provided"),
            Some(x) => std::ffi::OsStr::to_str(x).expect("Issue to unpack the file"),
        };
        match ext {
            "pfm" => Bitmap::read_pfm(filename),
            "exr" => Bitmap::read_exr(filename),
            _ => {
                // Try the default implementation support
                Bitmap::read_ldr_image(filename)
            }
        }
    }
}
// By default, create a black image
impl Default for Bitmap {
    fn default() -> Self {
        Bitmap {
            size: Vector2::new(1, 1),
            colors: vec![Color::zero()],
        }
    }
}

/// Ray representation
#[derive(Clone, Copy)]
pub struct Ray {
    pub o: Point3<f32>,
    pub d: Vector3<f32>,
    pub tnear: f32,
    pub tfar: f32,
}

impl Ray {
    pub fn new(o: Point3<f32>, d: Vector3<f32>) -> Ray {
        Ray {
            o,
            d,
            tnear: constants::EPSILON,
            tfar: std::f32::MAX,
        }
    }
}

// Some function based on vectors
fn vec_min(v1: &Vector3<f32>, v2: &Vector3<f32>) -> Vector3<f32> {
    Vector3::new(v1.x.min(v2.x), v1.y.min(v2.y), v1.z.min(v2.z))
}

fn vec_max(v1: &Vector3<f32>, v2: &Vector3<f32>) -> Vector3<f32> {
    Vector3::new(v1.x.max(v2.x), v1.y.max(v2.y), v1.z.max(v2.z))
}

fn vec_div(v1: &Vector3<f32>, v2: &Vector3<f32>) -> Vector3<f32> {
    Vector3::new(v1.x / v2.x, v1.y / v2.y, v1.z / v2.z)
}

fn vec_mult(v1: &Vector3<f32>, v2: &Vector3<f32>) -> Vector3<f32> {
    Vector3::new(v1.x * v2.x, v1.y * v2.y, v1.z * v2.z)
}

fn vec_max_coords(v: Vector3<f32>) -> f32 {
    v.x.max(v.y.max(v.z))
}

fn vec_min_coords(v: Vector3<f32>) -> f32 {
    v.x.min(v.y.min(v.z))
}

#[derive(Debug)]
pub struct AABB {
    pub p_min: Vector3<f32>,
    pub p_max: Vector3<f32>,
}

impl Default for AABB {
    fn default() -> Self {
        Self {
            p_min: Vector3::new(std::f32::MAX, std::f32::MAX, std::f32::MAX),
            p_max: Vector3::new(std::f32::MIN, std::f32::MIN, std::f32::MIN),
        }
    }
}

impl AABB {
    pub fn union_aabb(&self, b: &AABB) -> AABB {
        AABB {
            p_min: vec_min(&self.p_min, &b.p_min),
            p_max: vec_max(&self.p_max, &b.p_max),
        }
    }

    pub fn union_vec(&self, v: &Vector3<f32>) -> AABB {
        AABB {
            p_min: vec_min(&self.p_min, v),
            p_max: vec_max(&self.p_max, v),
        }
    }

    pub fn size(&self) -> Vector3<f32> {
        self.p_max - self.p_min
    }

    pub fn center(&self) -> Vector3<f32> {
        self.size() * 0.5 + self.p_min
    }

    pub fn intersect(&self, r: &Ray) -> Option<f32> {
        // TODO: direction inverse could be precomputed
        let t_0 = vec_div(&(self.p_min - r.o.to_vec()), &r.d);
        let t_1 = vec_div(&(self.p_max - r.o.to_vec()), &r.d);
        let t_min = vec_max_coords(vec_min(&t_0, &t_1));
        let t_max = vec_min_coords(vec_max(&t_0, &t_1));
        if t_min <= t_max {
            // FIXME: Maybe wrong if tmin is different
            if t_min >= r.tfar {
                None
            } else {
                Some(t_min)
            }
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct Intersection<'a> {
    /// Intersection distance
    pub dist: f32,
    /// Geometry normal
    pub n_g: Vector3<f32>,
    /// Shading normal
    pub n_s: Vector3<f32>,
    /// Intersection point
    pub p: Point3<f32>,
    /// Textures coordinates
    pub uv: Option<Vector2<f32>>,
    /// Mesh which we have intersected
    pub mesh: &'a Mesh,
    /// Frame from the intersection point
    pub frame: Frame,
    /// Incomming direction in the local coordinates
    pub wi: Vector3<f32>,
}

impl<'a> Intersection<'a> {
    pub fn cos_theta(&self) -> f32 {
        self.wi.z
    }
    pub fn to_local(&self, d: &Vector3<f32>) -> Vector3<f32> {
        self.frame.to_local(*d)
    }
    pub fn to_world(&self, d: &Vector3<f32>) -> Vector3<f32> {
        self.frame.to_world(*d)
    }
}

#[derive(Clone, Debug, Copy)]
pub struct VarianceEstimator {
    pub mean: f32,
    pub mean_sqr: f32,
    pub sample_count: u32,
}
impl VarianceEstimator {
    fn add(&mut self, v: f32) {
        self.sample_count += 1;
        let delta = v - self.mean;
        self.mean += delta / self.sample_count as f32;
        self.mean_sqr += delta * (v - self.mean);
    }

    fn variance(&self) -> f32 {
        self.mean_sqr / (self.sample_count - 1) as f32
    }
}
impl Default for VarianceEstimator {
    fn default() -> Self {
        Self {
            mean: 0.0,
            mean_sqr: 0.0,
            sample_count: 0,
        }
    }
}
