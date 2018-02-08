extern crate cgmath;
extern crate image;
extern crate tobj;
extern crate rayon;
extern crate serde_json;
extern crate embree;
extern crate rustlight;

use cgmath::*;
use image::*;
use std::time::Instant;
use std::io::prelude::*;
use serde_json::Value;
// rustlight uses
use rustlight::*;
use rustlight::structure::*;
use rustlight::geometry::*;

fn load_obj(scene: &mut embree::rtcore::Scene, file_name: &std::path::Path) -> Result<Vec<Mesh>, tobj::LoadError> {
    println!("Try to load {:?}", file_name);
    let (models, materials) = tobj::load_obj(file_name)?;

    // Read models
    let mut meshes = vec![];
    for m in models {
        println!("Loading model {}", m.name);
        let mesh = m.mesh;
        println!("{} has {} triangles", m.name, mesh.indices.len() / 3);
        let verts = mesh.positions.chunks(3).map(|i| Vector4::new(i[0], i[1], i[2], 1.0)).collect();
        let trimesh = scene.new_triangle_mesh(embree::rtcore::GeometryFlags::Static,
                                              verts,
                                              mesh.indices);
        // Read materials
        let diffuse_color;
        if let Some(id) = mesh.material_id {
            println!("found bsdf id: {}", id);
            let mat = &materials[id];
            diffuse_color = Color::new(mat.diffuse[0],
                                       mat.diffuse[1],
                                       mat.diffuse[2]);
        } else {
            diffuse_color = Color::one(0.0);
        }

        // Add the mesh info
        meshes.push(Mesh {
            name: m.name,
            trimesh,
            bsdf: diffuse_color,
            emission: Color::one(0.0),
        })
    }
    Ok(meshes)
}

fn main() {

    // Read the scene file
    let scene_path = std::path::Path::new("./data/cbox.json");
    let mut fscene = std::fs::File::open(scene_path).expect("scene file not found");
    let mut scene_str = String::new();
    fscene.read_to_string(&mut scene_str).expect("impossible to read the file");
    let v: Value = serde_json::from_str(&scene_str).expect("impossible to parse in JSON");

    let camera_param: camera::CameraParam = serde_json::from_value(v["camera"].clone()).unwrap();

    // Prepare embree
    let mut device = embree::rtcore::Device::new();
    let mut scene_embree = device.new_scene(embree::rtcore::STATIC,
                                            embree::rtcore::INTERSECT1 | embree::rtcore::INTERPOLATE);

    // Read the object
    let obj_path_str = v["meshes"].as_str().unwrap();
    let obj_path = scene_path.parent().unwrap().join(obj_path_str);
    let mut meshes = load_obj(&mut scene_embree, obj_path.as_path()).unwrap();

    // Add light if needed
    if let Some(emitters_json) = v.get("emitters") {
        for e in emitters_json.as_array().unwrap().iter() {
            let name: String = serde_json::from_value(e["mesh"].clone()).unwrap();
            let emission: Color = serde_json::from_value(e["emission"].clone()).unwrap();

            let mut found = false;
            for m in meshes.iter_mut() {
                if m.name == name {
                    m.emission = emission.clone();
                    found = true;
                }
            }

            if !found {
                panic!("Not found {} in the obj list", name);
            } else {
                println!("Ligth {:?} emission created", emission);
            }
        }
    }

    println!("Build the acceleration structure");
    scene_embree.commit(); // Build

    // Define a default scene
    let scene = scene::Scene {
        camera: camera::Camera::new(camera_param),
        embree: &scene_embree,
        meshes,
        nb_samples: 128,
    };

    println!("Rendering...");
    // To time the rendering time
    let start = Instant::now();
    // Generate the thread pool
    let pool = rayon::ThreadPool::new(rayon::Configuration::new()).unwrap();
    // Render the image
    let img: DynamicImage = pool.install(|| scene::render(&scene));

    assert_eq!(scene.camera.size().x, img.width());
    assert_eq!(scene.camera.size().y, img.height());
    // Compute the rendering time
    let elapsed = start.elapsed();
    println!("Elapsed: {} ms",
             (elapsed.as_secs() * 1_000) + (elapsed.subsec_nanos() / 1_000_000) as u64);
    // Save the image
    let ref mut fout = std::fs::File::create("test.png").unwrap();
    img.save(fout, image::PNG).expect("failed to write img into file");
}
