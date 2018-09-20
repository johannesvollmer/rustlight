use cgmath::Point2;
use integrators::{gradient::*, *};
use paths::path::*;
use paths::vertex::*;
use samplers::*;
use std::cell::RefCell;
use std::rc::Rc;
use structure::*;

// This special random number replay
// can capture the underlying sampler
// in order to replay the sequence of random number
// if it is necessary
pub struct ReplaySampler<'sampler, 'seq> {
    pub sampler: &'sampler mut Sampler,
    pub random: &'seq mut Vec<f32>,
    pub indice: usize,
}
impl<'sampler, 'seq> ReplaySampler<'sampler, 'seq> {
    fn generate(&mut self) -> f32 {
        assert!(self.indice <= self.random.len());
        if self.indice < self.random.len() {
            let v = self.indice;
            self.indice += 1;
            self.random[v]
        } else {
            let v = self.sampler.next();
            self.indice += 1;
            self.random.push(v);
            v
        }
    }

    pub fn unregistered(&mut self) -> f32 {
        self.sampler.next()
    }
}
impl<'sampler, 'seq> Sampler for ReplaySampler<'sampler, 'seq> {
    fn next(&mut self) -> f32 {
        self.generate()
    }
    fn next2d(&mut self) -> Point2<f32> {
        let v1 = self.generate();
        let v2 = self.generate();
        Point2::new(v1, v2)
    }
}
pub struct ShiftRandomReplay {
    pub random_sequence: Vec<f32>,
}
impl Default for ShiftRandomReplay {
    fn default() -> Self {
        Self {
            random_sequence: vec![],
        }
    }
}

/// Path tracing system
/// This structure store the rendering options
/// That the user have given through the command line
pub struct IntegratorGradientPathTracing {
    pub max_depth: Option<u32>,
    pub recons: Box<PoissonReconstruction + Sync>,
    pub min_survival: Option<f32>,
}
/// This structure is responsible to the graph generation
pub struct TechniqueGradientPathTracing {
    pub max_depth: Option<u32>,
    pub samplings: Vec<Box<SamplingStrategy>>,
    pub img_pos: Point2<u32>,
}
impl<'a> Technique<'a> for TechniqueGradientPathTracing {
    fn init(
        &mut self,
        scene: &'a Scene,
        sampler: &mut Sampler,
    ) -> Vec<(Rc<RefCell<Vertex<'a>>>, Color)> {
        // Only generate a path from the sensor
        let root = Rc::new(RefCell::new(Vertex::Sensor(SensorVertex {
            uv: Point2::new(
                self.img_pos.x as f32 + sampler.next(),
                self.img_pos.y as f32 + sampler.next(),
            ),
            pos: scene.camera.position(),
            edge_in: None,
            edge_out: None,
        })));

        return vec![(root, Color::one())];
    }

    fn expand(&self, _vertex: &Rc<RefCell<Vertex<'a>>>, depth: u32) -> bool {
        self.max_depth.map_or(true, |max| depth < max)
    }

    fn strategies(&self, _vertex: &Rc<RefCell<Vertex<'a>>>) -> &Vec<Box<SamplingStrategy>> {
        &self.samplings
    }
}
impl TechniqueGradientPathTracing {
    fn evaluate<'a>(&self, scene: &'a Scene, vertex: &Rc<VertexPtr<'a>>) -> Color {
        let mut l_i = Color::zero();
        match *vertex.borrow() {
            Vertex::Surface(ref v) => {
                for edge in &v.edge_out {
                    let contrib = edge.borrow().contribution();
                    if !contrib.is_zero() {
                        let weight = if let PDF::SolidAngle(v) = edge.borrow().pdf_direction {
                            let total: f32 = self
                                .strategies(vertex)
                                .iter()
                                .map(|s| {
                                    if let Some(v) = s.pdf(scene, &vertex, edge) {
                                        v
                                    } else {
                                        0.0
                                    }
                                }).sum();
                            v / total
                        } else {
                            1.0
                        };
                        l_i += contrib * weight;
                    }

                    let edge = edge.borrow();
                    if let Some(ref vertex_next) = edge.vertices.1 {
                        l_i += edge.weight * edge.rr_weight * self.evaluate(scene, &vertex_next);
                    }
                }
            }
            Vertex::Sensor(ref v) => {
                // Only one strategy where...
                let edge = v.edge_out.as_ref().unwrap();

                // Get the potential contribution
                let contrib = edge.borrow().contribution();
                if !contrib.is_zero() {
                    l_i += contrib;
                }

                // Do the reccursive call
                if let Some(ref vertex_next) = edge.borrow().vertices.1 {
                    l_i += edge.borrow().weight * self.evaluate(scene, &vertex_next);
                }
            }
            _ => {}
        };
        l_i
    }
}
impl Integrator for IntegratorGradientPathTracing {}
impl IntegratorGradient for IntegratorGradientPathTracing {
    fn reconstruct(&self) -> &Box<PoissonReconstruction + Sync> {
        &self.recons
    }

    fn compute_gradients(&mut self, scene: &Scene) -> Bitmap {
        let (nb_buffers, buffernames, mut image_blocks, ids) =
            generate_img_blocks_gradient(scene, &self.recons);

        let progress_bar = Mutex::new(ProgressBar::new(image_blocks.len() as u64));
        let pool = generate_pool(scene);
        pool.install(|| {
            image_blocks.par_iter_mut().for_each(|(info, im_block)| {
                let mut sampler = independent::IndependentSampler::default();
                for ix in info.x_pos_off..im_block.size.x - info.x_size_off {
                    for iy in info.y_pos_off..im_block.size.y - info.y_size_off {
                        for n in 0..scene.nb_samples() {
                            let c = self.compute_pixel(
                                (ix + im_block.pos.x, iy + im_block.pos.y),
                                scene,
                                &mut sampler,
                            );
                            // Accumulate the values inside the buffer
                            let pos = Point2::new(ix, iy);
                            let offset_buffers = (n % nb_buffers) * 3; // 3 buffers are in multiple version
                            im_block.accumulate(
                                pos,
                                c.main,
                                &buffernames[ids.primal + offset_buffers],
                            );
                            im_block.accumulate(
                                pos,
                                c.very_direct,
                                &buffernames[ids.very_direct].to_owned(),
                            );
                            for i in 0..4 {
                                // primal reuse
                                let off = GRADIENT_ORDER[i];
                                let pos_off = Point2::new(ix as i32 + off.x, iy as i32 + off.y);
                                im_block.accumulate_safe(
                                    pos_off,
                                    c.radiances[i],
                                    &buffernames[ids.primal + offset_buffers],
                                );
                                // gradient
                                match GRADIENT_DIRECTION[i] {
                                    GradientDirection::X(v) => match v {
                                        1 => im_block.accumulate(
                                            pos,
                                            c.gradients[i],
                                            &buffernames[ids.gradient_x + offset_buffers],
                                        ),
                                        -1 => im_block.accumulate_safe(
                                            pos_off,
                                            c.gradients[i] * -1.0,
                                            &buffernames[ids.gradient_x + offset_buffers],
                                        ),
                                        _ => panic!("wrong displacement X"), // FIXME: Fix the enum
                                    },
                                    GradientDirection::Y(v) => match v {
                                        1 => im_block.accumulate(
                                            pos,
                                            c.gradients[i],
                                            &buffernames[ids.gradient_y + offset_buffers],
                                        ),
                                        -1 => im_block.accumulate_safe(
                                            pos_off,
                                            c.gradients[i] * -1.0,
                                            &buffernames[ids.gradient_y + offset_buffers],
                                        ),
                                        _ => panic!("wrong displacement Y"),
                                    },
                                }
                            }
                        }
                    }
                }
                im_block.scale(1.0 / (scene.nb_samples() as f32));
                // Renormalize correctly the buffer informations
                for i in 0..nb_buffers {
                    let offset_buffers = i * 3; // 3 buffer that have multiple entries
                                                // 4 strategies as reuse primal
                    im_block.scale_buffer(
                        0.25 * nb_buffers as f32,
                        &buffernames[ids.primal + offset_buffers],
                    );
                    im_block.scale_buffer(
                        nb_buffers as f32,
                        &buffernames[ids.gradient_x + offset_buffers],
                    );
                    im_block.scale_buffer(
                        nb_buffers as f32,
                        &buffernames[ids.gradient_y + offset_buffers],
                    );
                }

                {
                    progress_bar.lock().unwrap().inc();
                }
            });
        });

        // Fill the image & do the reconstruct
        let mut image = Bitmap::new(Point2::new(0, 0), *scene.camera.size(), &buffernames);
        for (_, im_block) in &image_blocks {
            image.accumulate_bitmap(im_block);
        }
        image
    }
}
impl IntegratorGradientPathTracing {
    fn compute_pixel(
        &self,
        (ix, iy): (u32, u32),
        scene: &Scene,
        sampler: &mut Sampler,
    ) -> ColorGradient {
        // Initialize the technique
        let mut samplings: Vec<Box<SamplingStrategy>> = Vec::new();
        samplings.push(Box::new(DirectionalSamplingStrategy {}));
        samplings.push(Box::new(LightSamplingStrategy {}));
        let mut technique = TechniqueGradientPathTracing {
            max_depth: self.max_depth,
            samplings,
            img_pos: Point2::new(ix, iy),
        };
        let mut random_sequence = vec![];
        let mut capture_sampler = ReplaySampler {
            sampler,
            random: &mut random_sequence,
            indice: 0,
        };
        // Call the generator on this technique
        // the generator give back the root nodes
        let root = generate(scene, &mut capture_sampler, &mut technique);
        let root_value = technique.evaluate(scene, &root[0].0);
        let weight_survival = if let Some(min_survival) = self.min_survival {
            // TODO: Change the 0.1 hard coded to a more meaningful value
            let prob_survival = (root_value.luminance() / 0.1).min(1.0).max(min_survival);
            if prob_survival == 1.0 || prob_survival >= capture_sampler.unregistered() {
                1.0 / prob_survival
            } else {
                0.0
            }
        } else {
            1.0
        };

        if weight_survival != 0.0 {
            let mut output = ColorGradient {
                very_direct: Color::zero(),
                main: root_value * 4.0 * 0.5 * weight_survival,
                radiances: [Color::zero(); 4],
                gradients: [Color::zero(); 4],
            };

            GRADIENT_ORDER.iter().enumerate().for_each(|(i, off)| {
                let pix = Point2::new(ix as i32 + off.x, iy as i32 + off.y);
                if pix.x < 0
                    || pix.x > scene.camera.size().x as i32
                    || pix.y < 0
                    || pix.y > scene.camera.size().y as i32
                {
                    // Do nothing
                } else {
                    // Change the pixel for the sampling technique
                    // and reset the sampler
                    technique.img_pos = Point2::new(pix.x as u32, pix.y as u32);
                    capture_sampler.indice = 0;
                    let offset_value = {
                        let offset = generate(scene, &mut capture_sampler, &mut technique);
                        technique.evaluate(scene, &offset[0].0)
                    };
                    output.radiances[i] = offset_value * 0.5 * weight_survival;
                    output.gradients[i] = (offset_value - root_value) * 0.5 * weight_survival;
                }
            });
            output
        } else {
            ColorGradient::default()
        }
    }
}
