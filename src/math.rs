use cgmath::*;

// This code have been taking from PBRT rust version
pub const INV_PI: f32 = 0.31830988618379067154;
pub const INV_2_PI: f32 = 0.15915494309189533577;
pub const PI_OVER_2: f32 = 1.57079632679489661923;
pub const PI_OVER_4: f32 = 0.78539816339744830961;
pub fn concentric_sample_disk(u: Point2<f32>) -> Point2<f32> {
    // map uniform random numbers to $[-1,1]^2$
    let u_offset: Point2<f32> = u * 2.0 as f32 - Vector2 { x: 1.0, y: 1.0 };
    // handle degeneracy at the origin
    if u_offset.x == 0.0 as f32 && u_offset.y == 0.0 as f32 {
        return Point2 { x : 0.0, y: 0.0 };
    }
    // apply concentric mapping to point
    let theta: f32;
    let r: f32;
    if u_offset.x.abs() > u_offset.y.abs() {
        r = u_offset.x;
        theta = PI_OVER_4 * (u_offset.y / u_offset.x);
    } else {
        r = u_offset.y;
        theta = PI_OVER_2 - PI_OVER_4 * (u_offset.x / u_offset.y);
    }
    Point2 {
        x: theta.cos(),
        y: theta.sin(),
    } * r
}
pub fn cosine_sample_hemisphere(u: Point2<f32>) -> Vector3<f32> {
    let d: Point2<f32> = concentric_sample_disk(u);
    let z: f32 = (0.0 as f32)
        .max(1.0 as f32 - d.x * d.x - d.y * d.y)
        .sqrt();
    Vector3 {
        x: d.x,
        y: d.y,
        z: z,
    }
}

/// Create an orthogonal basis by taking the normal vector
/// code based on Pixar paper.
pub fn basis(n : Vector3<f32>) -> Matrix3<f32> {
    // TODO: See how to use branchless version (copysignf)
    let b1: Vector3<f32>;
    let b2: Vector3<f32>;
    if n.z<0.0 {
        let a = 1.0 / (1.0 - n.z);
        let b = n.x * n.y * a;
        b1 = Vector3::new(1.0 - n.x * n.x * a, -b, n.x);
        b2 = Vector3::new(b, n.y * n.y*a - 1.0, -n.y);
    } else{
        let a = 1.0 / (1.0 + n.z);
        let b = -n.x * n.y * a;
        b1 = Vector3::new(1.0 - n.x * n.x * a, b, -n.x);
        b2 = Vector3::new(b, 1.0 - n.y * n.y * a, -n.y);
    }
    Matrix3 {
        x : b1,
        y : b2,
        z : n
    }
}

/// Uniformly distributing samples over isosceles right triangles
/// actually works for any triangle.
pub fn uniform_sample_triangle(u: (f32,f32)) -> Point2<f32> {
    let su0: f32 = u.0.sqrt();
    Point2 {
        x: 1.0 as f32 - su0,
        y: u.1 * su0,
    }
}


/// Create 1D distribution
pub struct Distribution1DConstruct {
    pub elements: Vec<f32>
}

pub struct Distribution1D {
    pub cdf: Vec<f32>,
    pub normalization: f32,
}

impl Distribution1DConstruct {
    pub fn new(l : usize) -> Distribution1DConstruct {
        let elements = Vec::with_capacity(l);
        Distribution1DConstruct {
            elements
        }
    }

    pub fn add(&mut self, v : f32) {
        self.elements.push(v);
    }

    pub fn normalize(&mut self) -> Distribution1D {
        // Create the new CDF
        let mut cdf = Vec::with_capacity(self.elements.len() + 1);
        let mut cur = 0.0;
        for e in self.elements.iter() {
            cdf.push(cur);
            cur += e;
        }
        cdf.push(cur);

        // Normalize the cdf
        cdf.iter_mut().for_each(|x| *x /= cur);

        Distribution1D {
            cdf ,
            normalization: cur,
        }
    }
}

impl Distribution1D {
    pub fn sample(&self, v: f32) -> usize {
        assert!(v >= 0.0);
        assert!(v < 1.0);

        match self.cdf.binary_search_by(|probe| probe.partial_cmp(&v).unwrap()) {
            Ok(x) => x,
            Err(x) => x - 1
        }
    }

    pub fn pdf(&self, i : usize) -> f32 {
        assert!(i < self.cdf.len() - 1);
        self.cdf[i+1] - self.cdf[i]
    }
}