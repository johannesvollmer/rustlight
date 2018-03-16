use std::cmp;
use std::u32;
use std;
use std::sync::Arc;
use std::error::Error;

use rayon::prelude::*;
use cgmath::*;
use embree_rs;
use serde_json;

// my includes
use structure::{Color, Ray};
use camera::{Camera, CameraParam};
use integrator::*;
use geometry;
use tools::StepRangeInt;
use sampler;
use math::{Distribution1DConstruct,Distribution1D};
use material::*;

/// Image block
/// for easy paralelisation over the threads
pub struct Bitmap {
    pub pos: Point2<u32>,
    pub size: Vector2<u32>,
    pub pixels: Vec<Color>,
}

impl Bitmap {
    pub fn new(pos: Point2<u32>, size: Vector2<u32>) -> Bitmap {
        Bitmap {
            pos,
            size,
            pixels: vec![Color { r: 0.0, g: 0.0, b: 0.0 };
                         (size.x * size.y) as usize],
        }
    }

    pub fn accum_bitmap(&mut self, o: &Bitmap) {
        for x in 0..o.size.x {
            for y in 0..o.size.y {
                let c_p = Point2::new(o.pos.x + x, o.pos.y + y);
                self.accum(c_p, o.get(Point2::new(x, y)));
            }
        }
    }

    pub fn accum(&mut self, p: Point2<u32>, f: &Color) {
        assert!(p.x < self.size.x);
        assert!(p.y < self.size.y);
        self.pixels[(p.y * self.size.y + p.x) as usize] += f;
    }

    pub fn get(&self, p: Point2<u32>) -> &Color {
        assert!(p.x < self.size.x);
        assert!(p.y < self.size.y);
        &self.pixels[(p.y * self.size.y + p.x) as usize]
    }

    pub fn weight(&mut self, f: f32) {
        assert!(f > 0.0);
        self.pixels.iter_mut().for_each(|v| v.mul(f));
    }
}

/// Light sample representation
pub struct LightSampling<'a> {
    pub emitter : &'a geometry::Mesh,
    pub pdf : f32,
    pub p : Point3<f32>,
    pub n : Vector3<f32>,
    pub d : Vector3<f32>,
    pub weight : Color,
}

impl<'a> LightSampling<'a> {
    pub fn is_valid(&'a self) -> bool {
        self.pdf != 0.0
    }
}

/// Scene representation
pub struct Scene<'a> {
    /// Main camera
    pub camera: Camera,
    // Geometry information
    pub meshes: Vec<Arc<geometry::Mesh>>,
    pub emitters: Vec<Arc<geometry::Mesh>>,
    emitters_cdf: Distribution1D,
    #[allow(dead_code)]
    embree_device: embree_rs::scene::Device<'a>,
    embree_scene: embree_rs::scene::Scene<'a>,
    // Integrator
    integrator : Box<Integrator + Send + Sync>
}

impl<'a> Scene<'a> {
    /// Take a json formatted string and an working directory
    /// and build the scene representation.
    pub fn new(data: &str, wk: &std::path::Path, int: Box<Integrator + Send + Sync>) -> Result<Scene<'a>, Box<Error>> {
        // Read json string
        let v: serde_json::Value = serde_json::from_str(data)?;

        // Allocate embree
        let mut device = embree_rs::scene::Device::new();
        let mut scene_embree = device.new_scene(embree_rs::scene::SceneFlags::STATIC,
                                                embree_rs::scene::AlgorithmFlags::INTERSECT1);

        // Read the object
        let obj_path_str: String = v["meshes"].as_str().unwrap().to_string();
        let obj_path = wk.join(obj_path_str);
        let mut meshes = geometry::load_obj(&mut scene_embree, obj_path.as_path())?;

        // Build embree as we will not geometry for now
        println!("Build the acceleration structure");
        scene_embree.commit(); // Build

        // Update meshes informations
        //  - which are light?
        if let Some(emitters_json) = v.get("emitters") {
            for e in emitters_json.as_array().unwrap() {
                let name: String = e["mesh"].as_str().unwrap().to_string();
                let emission: Color = serde_json::from_value(e["emission"].clone())?;
                // Get the set of matched meshes
                let mut matched_meshes = meshes.iter_mut().filter(|m| m.name == name).collect::<Vec<_>>();
                match matched_meshes.len() {
                    0 =>  panic!("Not found {} in the obj list", name),
                    1 => {
                        matched_meshes[0].emission = emission;
                    },
                    _ => panic!("Several {} in the obj list", name),
                };
            }
        }
        // - BSDF
        if let Some(bsdfs_json) = v.get("bsdfs") {
            for b in bsdfs_json.as_array().unwrap() {
                let name: String = serde_json::from_value(b["mesh"].clone())?;
                let new_bsdf_type: String = serde_json::from_value(b["type"].clone())?;
                let new_bsdf: Box<BSDF + Send + Sync> = match new_bsdf_type.as_ref() {
                    "phong" => Box::<BSDFPhong>::new(serde_json::from_value(b["data"].clone())?),
                    "diffuse" => Box::<BSDFDiffuse>::new(serde_json::from_value(b["data"].clone())?),
                    _ => panic!("Unknown BSDF type {}", new_bsdf_type),
                };

                let mut matched_meshes = meshes.iter_mut().filter(|m| m.name == name).collect::<Vec<_>>();
                match matched_meshes.len() {
                    0 =>  panic!("Not found {} in the obj list", name),
                    1 => {
                        matched_meshes[0].bsdf = new_bsdf;
                    },
                    _ => panic!("Several {} in the obj list", name),
                };
            }
        }
        
        // Transform the scene mesh from Box to Arc
        let meshes: Vec<Arc<geometry::Mesh>> = meshes.into_iter().map(|e| Arc::from(e)).collect();

        // Update the list of lights & construct the CDF
        let emitters = meshes.iter().filter(|m| !m.emission.is_zero())
            .map(|m| m.clone()).collect::<Vec<_>>();
        let emitters_cdf = {
            let mut cdf_construct = Distribution1DConstruct::new(emitters.len());
            emitters.iter().map(|e| e.flux()).for_each(|f| cdf_construct.add(f));
            cdf_construct.normalize()
        };

        // Read the camera config
        let camera_param: CameraParam = serde_json::from_value(v["camera"].clone()).unwrap();

        // Define a default scene
        Ok(Scene {
            camera: Camera::new(camera_param),
            embree_device: device,
            embree_scene: scene_embree,
            meshes,
            emitters,
            emitters_cdf,
            integrator : int,
        })
    }

    /// Intersect and compute intersection informations
    pub fn trace(&self, ray: &Ray) -> Option<embree_rs::ray::Intersection> {
        let embree_ray = embree_rs::ray::Ray::new(
            &ray.o, &ray.d,
            ray.tnear, ray.tfar);
        self.embree_scene.intersect(embree_ray)
    }

    /// Intersect the scene and return if we had an intersection or not
    pub fn hit(&self, ray: &Ray) -> bool {
        let mut embree_ray = embree_rs::ray::Ray::new(
            &ray.o, &ray.d,
            ray.tnear, ray.tfar);
        self.embree_scene.occluded(&mut embree_ray);
        embree_ray.hit()
    }

    pub fn visible(&self, p0: &Point3<f32>, p1: &Point3<f32>) -> bool {
        let d = p1 - p0;
        let mut embree_ray = embree_rs::ray::Ray::new(
            p0, &d, 0.00001, 0.9999);
        self.embree_scene.occluded(&mut embree_ray);
        !embree_ray.hit()
    }

    pub fn direct_pdf(&self, ray: &Ray, its: &embree_rs::ray::Intersection) -> f32 {
        let mesh = &self.meshes[its.geom_id as usize];
        let emitter_id = self.emitters.iter().position(|m| Arc::ptr_eq(mesh,m)).unwrap();
        mesh.direct_pdf(ray, its) * self.emitters_cdf.pdf(emitter_id)
    }
    pub fn sample_light(&self, p: &Point3<f32>, r_sel: f32, r: f32, uv: Point2<f32>) -> LightSampling {
        // Select the point on the light
        let (pdf_sel, emitter) = self.random_select_emitter(r_sel);
        let sampled_pos = emitter.sample(r, uv);

        // Compute the distance
        let mut d: Vector3<f32> = sampled_pos.p - p;
        let dist = d.magnitude();
        d /= dist;

        // Compute the geometry
        let geom_light = sampled_pos.n.dot(-d).max(0.0) / (dist * dist);
        let emission = emitter.emission.clone() * (geom_light / (pdf_sel * sampled_pos.pdf));
        LightSampling {
            emitter,
            pdf : if geom_light == 0.0 {0.0} else {sampled_pos.pdf * pdf_sel * ( 1.0 / geom_light )},
            p: sampled_pos.p,
            n: sampled_pos.n,
            d,
            weight: emission,
        }

    }
    pub fn random_select_emitter(&self, v: f32) -> (f32, &geometry::Mesh) {
        let id_light = self.emitters_cdf.sample(v);
        (self.emitters_cdf.pdf(id_light), &self.emitters[id_light])
    }

    /// Render the scene
    pub fn render(&self, nb_samples: u32) -> Bitmap {
        assert!(nb_samples != 0);

        // Create rendering blocks
        let mut image_blocks: Vec<Bitmap> = Vec::new();
        for ix in StepRangeInt::new(0, self.camera.size().x as usize, 16) {
            for iy in StepRangeInt::new(0, self.camera.size().y as usize, 16) {
                let mut block = Bitmap::new(
                    Point2 { x: ix as u32, y: iy as u32 },
                    Vector2 {
                        x: cmp::min(16, self.camera.size().x - ix as u32),
                        y: cmp::min(16, self.camera.size().y - iy as u32),
                    });
                image_blocks.push(block);
            }
        }

        // Render the image blocks
        image_blocks.par_iter_mut().for_each(|im_block|
            {
                let mut sampler = sampler::IndepSampler::default();
                for ix in 0..im_block.size.x {
                    for iy in 0..im_block.size.y {
                        for _ in 0..nb_samples {
                            let c = self.integrator.compute((ix + im_block.pos.x, iy + im_block.pos.y),
                                                            self, &mut sampler);
                            im_block.accum(Point2 { x: ix, y: iy }, &c);
                        }
                    }
                }
                im_block.weight(1.0 / (nb_samples as f32));
            }
        );

        // Fill the image
        let mut image = Bitmap::new(Point2::new(0, 0), self.camera.size().clone());
        for im_block in &image_blocks {
            image.accum_bitmap(im_block);
        }
        image
    }
}