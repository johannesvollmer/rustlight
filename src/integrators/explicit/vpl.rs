use crate::integrators::*;
use crate::paths::path::*;
use crate::paths::vertex::*;
use crate::samplers;
use crate::volume::*;
use cgmath::{EuclideanSpace, InnerSpace, Point2, Point3, Vector3};

pub struct IntegratorVPL {
    pub nb_vpl: usize,
    pub max_depth: Option<u32>,
    pub clamping_factor: Option<f32>,
}

struct VPLSurface<'a> {
    its: Intersection<'a>,
    radiance: Color,
}
struct VPLVolume {
    pos: Point3<f32>,
    d_in: Vector3<f32>,
    phase_function: PhaseFunction,
    radiance: Color,
}
struct VPLEmitter {
    pos: Point3<f32>,
    n: Vector3<f32>,
    emitted_radiance: Color,
}

enum VPL<'a> {
    Surface(VPLSurface<'a>),
    Volume(VPLVolume),
    Emitter(VPLEmitter),
}

pub struct TechniqueVPL {
    pub max_depth: Option<u32>,
    pub samplings: Vec<Box<dyn SamplingStrategy>>,
    pub flux: Option<Color>,
}

impl Technique for TechniqueVPL {
    fn init<'scene, 'emitter>(
        &mut self,
        path: &mut Path<'scene, 'emitter>,
        _accel: &dyn Acceleration,
        _scene: &'scene Scene,
        sampler: &mut dyn Sampler,
        emitters: &'emitter EmitterSampler,
    ) -> Vec<(VertexID, Color)> {
        let (emitter, sampled_point, flux) = emitters.random_sample_emitter_position(
            sampler.next(),
            sampler.next(),
            sampler.next2d(),
        );
        let emitter_vertex = Vertex::Light(EmitterVertex {
            pos: sampled_point.p,
            n: sampled_point.n,
            emitter,
            edge_in: None,
            edge_out: None,
        });
        self.flux = Some(flux); // Capture the scaled flux
        vec![(path.register_vertex(emitter_vertex), Color::one())]
    }

    fn expand(&self, _vertex: &Vertex, depth: u32) -> bool {
        self.max_depth.map_or(true, |max| depth < max)
    }

    fn strategies(&self, _vertex: &Vertex) -> &Vec<Box<dyn SamplingStrategy>> {
        &self.samplings
    }
}

impl TechniqueVPL {
    fn convert_vpl<'scene>(
        &self,
        path: &Path<'scene, '_>,
        scene: &'scene Scene,
        vertex_id: VertexID,
        vpls: &mut Vec<VPL<'scene>>,
        flux: Color,
    ) {
        match path.vertex(vertex_id) {
            Vertex::Surface(ref v) => {
                vpls.push(VPL::Surface(VPLSurface {
                    its: v.its.clone(),
                    radiance: flux,
                }));

                // Continue to bounce...
                for edge in &v.edge_out {
                    let edge = path.edge(*edge);
                    if let Some(vertex_next_id) = edge.vertices.1 {
                        self.convert_vpl(
                            path,
                            scene,
                            vertex_next_id,
                            vpls,
                            flux * edge.weight * edge.rr_weight,
                        );
                    }
                }
            }
            Vertex::Volume(ref v) => {
                vpls.push(VPL::Volume(VPLVolume {
                    pos: v.pos,
                    d_in: v.d_in,
                    phase_function: v.phase_function.clone(),
                    radiance: flux,
                }));

                // Continue to bounce...
                for edge in &v.edge_out {
                    let edge = path.edge(*edge);
                    if let Some(vertex_next_id) = edge.vertices.1 {
                        self.convert_vpl(
                            path,
                            scene,
                            vertex_next_id,
                            vpls,
                            flux * edge.weight * edge.rr_weight,
                        );
                    }
                }
            }
            Vertex::Light(ref v) => {
                let flux = *self.flux.as_ref().unwrap();
                vpls.push(VPL::Emitter(VPLEmitter {
                    pos: v.pos,
                    n: v.n,
                    emitted_radiance: flux,
                }));

                if let Some(edge) = v.edge_out {
                    let edge = path.edge(edge);
                    if let Some(next_vertex_id) = edge.vertices.1 {
                        self.convert_vpl(
                            path,
                            scene,
                            next_vertex_id,
                            vpls,
                            edge.weight * flux * edge.rr_weight,
                        );
                    }
                }
            }
            Vertex::Sensor(ref _v) => {}
        }
    }
}

impl Integrator for IntegratorVPL {
    fn compute(&mut self, accel: &dyn Acceleration, scene: &Scene) -> BufferCollection {
        info!("Generating the VPL...");
        let buffernames = vec![String::from("primal")];
        let mut sampler = samplers::independent::IndependentSampler::default();
        let mut nb_path_shot = 0;
        let mut vpls = vec![];
        let emitters = scene.emitters_sampler();
        while vpls.len() < self.nb_vpl as usize {
            let samplings: Vec<Box<dyn SamplingStrategy>> =
                vec![Box::new(DirectionalSamplingStrategy { from_sensor: false })];
            let mut technique = TechniqueVPL {
                max_depth: self.max_depth,
                samplings,
                flux: None,
            };
            let mut path = Path::default();
            let root = generate(
                &mut path,
                accel,
                scene,
                &emitters,
                &mut sampler,
                &mut technique,
            );
            technique.convert_vpl(&path, scene, root[0].0, &mut vpls, Color::one());
            nb_path_shot += 1;
        }
        let vpls = vpls;

        // Generate the image block to get VPL efficiently
        let mut image_blocks = generate_img_blocks(scene, &buffernames);

        // Render the image blocks VPL integration
        info!("Gathering VPL...");
        let progress_bar = Mutex::new(ProgressBar::new(image_blocks.len() as u64));
        let norm_vpl = 1.0 / nb_path_shot as f32;
        let pool = generate_pool(scene);
        pool.install(|| {
            image_blocks.par_iter_mut().for_each(|im_block| {
                let mut sampler = independent::IndependentSampler::default();
                for ix in 0..im_block.size.x {
                    for iy in 0..im_block.size.y {
                        for _ in 0..scene.nb_samples {
                            let c = self.compute_vpl_contrib(
                                (ix + im_block.pos.x, iy + im_block.pos.y),
                                accel,
                                scene,
                                &mut sampler,
                                &vpls,
                                norm_vpl,
                            );
                            im_block.accumulate(Point2 { x: ix, y: iy }, c, &"primal".to_owned());
                        }
                    }
                }
                im_block.scale(1.0 / (scene.nb_samples as f32));
                {
                    progress_bar.lock().unwrap().inc();
                }
            });
        });

        // Fill the image
        let mut image =
            BufferCollection::new(Point2::new(0, 0), *scene.camera.size(), &buffernames);
        for im_block in &image_blocks {
            image.accumulate_bitmap(im_block);
        }
        image
    }
}

impl IntegratorVPL {
    fn transmittance(
        &self,
        medium: Option<&HomogenousVolume>,
        p1: Point3<f32>,
        p2: Point3<f32>,
    ) -> Color {
        if let Some(m) = medium {
            let mut d = p2 - p1;
            let dist = d.magnitude();
            d /= dist;
            let mut r = Ray::new(p1, d);
            r.tfar = dist;
            m.transmittance(r)
        } else {
            Color::one()
        }
    }

    fn gathering_surface<'a>(
        &self,
        medium: Option<&HomogenousVolume>,
        accel: &dyn Acceleration,
        vpls: &[VPL<'a>],
        norm_vpl: f32,
        its: &Intersection,
    ) -> Color {
        let mut l_i = Color::zero();

        // Self emission
        if its.cos_theta() > 0.0 {
            l_i += &(its.mesh.emission);
        }

        for vpl in vpls {
            match *vpl {
                VPL::Emitter(ref vpl) => {
                    if accel.visible(&vpl.pos, &its.p) {
                        let mut d = vpl.pos - its.p;
                        let dist = d.magnitude();
                        d /= dist;

                        let emitted_radiance = vpl.emitted_radiance
                            * vpl.n.dot(-d).max(0.0)
                            * std::f32::consts::FRAC_1_PI;
                        if !its.mesh.bsdf.is_smooth() {
                            let bsdf_val = its.mesh.bsdf.eval(
                                &its.uv,
                                &its.wi,
                                &its.to_local(&d),
                                Domain::SolidAngle,
                            );
                            let trans = self.transmittance(medium, its.p, vpl.pos);
                            l_i += trans * norm_vpl * emitted_radiance * bsdf_val / (dist * dist);
                        }
                    }
                }
                VPL::Volume(ref vpl) => {
                    let mut d = vpl.pos - its.p;
                    let dist = d.magnitude();
                    d /= dist;

                    if !its.mesh.bsdf.is_smooth() {
                        let emitted_radiance = vpl.phase_function.eval(&vpl.d_in, &d);
                        let bsdf_val = its.mesh.bsdf.eval(
                            &its.uv,
                            &its.wi,
                            &its.to_local(&d),
                            Domain::SolidAngle,
                        );
                        let trans = self.transmittance(medium, its.p, vpl.pos);
                        l_i += trans * norm_vpl * emitted_radiance * bsdf_val * vpl.radiance
                            / (dist * dist);
                    }
                }
                VPL::Surface(ref vpl) => {
                    if accel.visible(&vpl.its.p, &its.p) {
                        let mut d = vpl.its.p - its.p;
                        let dist = d.magnitude();
                        d /= dist;

                        if !its.mesh.bsdf.is_smooth() {
                            let emitted_radiance = vpl.its.mesh.bsdf.eval(
                                &vpl.its.uv,
                                &vpl.its.wi,
                                &vpl.its.to_local(&-d),
                                Domain::SolidAngle,
                            );
                            let bsdf_val = its.mesh.bsdf.eval(
                                &its.uv,
                                &its.wi,
                                &its.to_local(&d),
                                Domain::SolidAngle,
                            );
                            let trans = self.transmittance(medium, its.p, vpl.its.p);
                            l_i += trans * norm_vpl * emitted_radiance * bsdf_val * vpl.radiance
                                / (dist * dist);
                        }
                    }
                }
            }
        }
        l_i
    }

    fn gathering_volume<'a>(
        &self,
        medium: Option<&HomogenousVolume>,
        accel: &dyn Acceleration,
        vpls: &[VPL<'a>],
        norm_vpl: f32,
        d_cam: Vector3<f32>,
        pos: Point3<f32>,
        phase: &PhaseFunction,
    ) -> Color {
        let mut l_i = Color::zero();
        for vpl in vpls {
            match *vpl {
                VPL::Emitter(ref vpl) => {
                    if accel.visible(&vpl.pos, &pos) {
                        let mut d = vpl.pos - pos;
                        let dist = d.magnitude();
                        d /= dist;

                        let emitted_radiance = vpl.emitted_radiance
                            * vpl.n.dot(-d).max(0.0)
                            * std::f32::consts::FRAC_1_PI;
                        let phase_val = phase.eval(&d_cam, &d);
                        let trans = self.transmittance(medium, pos, vpl.pos);
                        l_i += trans * norm_vpl * emitted_radiance * phase_val / (dist * dist);
                    }
                }
                VPL::Volume(ref vpl) => {
                    let mut d = vpl.pos - pos;
                    let dist = d.magnitude();
                    d /= dist;

                    let emitted_radiance = vpl.phase_function.eval(&vpl.d_in, &d);
                    let phase_val = phase.eval(&d_cam, &d);
                    let trans = self.transmittance(medium, pos, vpl.pos);
                    l_i += trans * norm_vpl * emitted_radiance * phase_val * vpl.radiance
                        / (dist * dist);
                }
                VPL::Surface(ref vpl) => {
                    if accel.visible(&vpl.its.p, &pos) {
                        let mut d = vpl.its.p - pos;
                        let dist = d.magnitude();
                        d /= dist;

                        let emitted_radiance = vpl.its.mesh.bsdf.eval(
                            &vpl.its.uv,
                            &vpl.its.wi,
                            &vpl.its.to_local(&-d),
                            Domain::SolidAngle,
                        );
                        let phase_val = phase.eval(&d_cam, &d);
                        let trans = self.transmittance(medium, pos, vpl.its.p);
                        l_i += trans * norm_vpl * emitted_radiance * phase_val * vpl.radiance
                            / (dist * dist);
                    }
                }
            }
        }
        l_i
    }

    fn compute_vpl_contrib<'a>(
        &self,
        (ix, iy): (u32, u32),
        accel: &dyn Acceleration,
        scene: &'a Scene,
        sampler: &mut dyn Sampler,
        vpls: &[VPL<'a>],
        norm_vpl: f32,
    ) -> Color {
        let pix = Point2::new(ix as f32 + sampler.next(), iy as f32 + sampler.next());
        let ray = scene.camera.generate(pix);
        let mut l_i = Color::zero();

        // Check if we have a intersection with the primary ray
        let its = match accel.trace(&ray) {
            Some(x) => x,
            None => {
                if let Some(m) = &scene.volume {
                    // Sample the participating media
                    let mrec = m.sample(&ray, sampler.next2d());
                    assert!(!mrec.exited);
                    let pos = Point3::from_vec(ray.o.to_vec() + ray.d * mrec.t);
                    let phase_function = PhaseFunction::Isotropic(); // FIXME:
                    l_i *= self.gathering_volume(
                        scene.volume.as_ref(),
                        accel,
                        vpls,
                        norm_vpl,
                        -ray.d,
                        pos,
                        &phase_function,
                    ) * mrec.w;
                    return l_i;
                } else {
                    return l_i;
                }
            }
        };

        if let Some(m) = &scene.volume {
            let mut ray_med = ray;
            ray_med.tfar = its.dist;
            let mrec = m.sample(&ray_med, sampler.next2d());
            if !mrec.exited {
                let pos = Point3::from_vec(ray.o.to_vec() + ray.d * mrec.t);
                let phase_function = PhaseFunction::Isotropic(); // FIXME:
                l_i += self.gathering_volume(
                    scene.volume.as_ref(),
                    accel,
                    vpls,
                    norm_vpl,
                    -ray.d,
                    pos,
                    &phase_function,
                ) * mrec.w;
                l_i
            } else {
                l_i += self.gathering_surface(scene.volume.as_ref(), accel, vpls, norm_vpl, &its)
                    * mrec.w;
                l_i
            }
        } else {
            l_i += self.gathering_surface(scene.volume.as_ref(), accel, vpls, norm_vpl, &its);
            l_i
        }
    }
}
