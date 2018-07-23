use integrators::gradient_domain::*;
use integrators::path_explicit::*;
use integrators::*;
use paths::shift_op::random_replay::*;
use paths::shift_path::*;
use paths::vertex::*;
use scene::*;

impl Integrator<ColorGradient> for IntegratorUniPath {
    fn compute<S: Sampler>(
        &self,
        (ix, iy): (u32, u32),
        scene: &Scene,
        sampler: &mut S,
    ) -> ColorGradient {
        let mut shift_op = ShiftRandomReplay::default();
        let base_path = shift_op.generate_base((ix, iy), scene, sampler, self.max_depth);
        if base_path.is_none() {
            return ColorGradient::default();
        }
        let base_path = base_path.unwrap();
        let base_path_img_pos = base_path.get_img_position();
        let shift_paths = GRADIENT_ORDER
            .iter()
            .map(|e| {
                shift_op.shift(
                    &base_path,
                    Point2::new(
                        base_path_img_pos.x + e.x as f32,
                        base_path_img_pos.y + e.y as f32,
                    ),
                    scene,
                    sampler,
                    self.max_depth,
                )
            })
            .collect::<Vec<Option<ShiftPath>>>();

        let mut l_i = ColorGradient::default();
        for (i, vertex) in base_path.vertices.iter().enumerate() {
            match vertex {
                &Vertex::Surface(ref v) => {
                    ///////////////////////////////
                    // Sample the light explicitly
                    let random_light_num = (sampler.next(), sampler.next(), sampler.next2d());
                    let light_record = scene.sample_light(
                        &v.its.p,
                        random_light_num.0,
                        random_light_num.1,
                        random_light_num.2,
                    );
                    let light_pdf = match light_record.pdf {
                        PDF::SolidAngle(v) => v,
                        _ => panic!("Unsupported light pdf type for pdf connection."),
                    };

                    // Do not consider visibility here
                    let d_out_local = v.its.frame.to_local(light_record.d);
                    if light_record.is_valid() && d_out_local.z > 0.0
                    // FIXME: This might be a problem
                    {
                        // Compute the contribution of direct lighting
                        if let PDF::SolidAngle(pdf_bsdf) =
                            v.its.mesh.bsdf.pdf(&v.its.uv, &v.its.wi, &d_out_local)
                        {
                            // Compute pdf values
                            let (base_pdf, base_info) = if scene.visible(&v.its.p, &light_record.p)
                            {
                                (
                                    light_pdf,
                                    (
                                        v.throughput
                                            * v.its.mesh.bsdf.eval(
                                                &v.its.uv,
                                                &v.its.wi,
                                                &d_out_local,
                                            )
                                            * light_record.weight,
                                        light_pdf + pdf_bsdf,
                                    ),
                                )
                            } else {
                                (0.0, (Color::zero(), 0.0))
                            };

                            // FIXME: Check if this condition is correct or not
                            if base_pdf != 0.0 {
                                // For the shift here the strategy is to replay the random numbers.
                                let shift_info = shift_paths
                                    .iter()
                                    .map(|shift_path| {
                                        if shift_path.is_none() {
                                            return (Color::zero(), 0.0);
                                        }
                                        let shift_path = shift_path.as_ref().unwrap();
                                        match shift_path.vertices.get(i) {
                                            None => (Color::zero(), 0.0),
                                            Some(&ShiftVertex::Surface(ref v)) => {
                                                let light_record = scene.sample_light(
                                                    &v.its.p,
                                                    random_light_num.0,
                                                    random_light_num.1,
                                                    random_light_num.2,
                                                );
                                                let d_out_local =
                                                    v.its.frame.to_local(light_record.d);
                                                if light_record.is_valid()
                                                    && d_out_local.z > 0.0
                                                    && scene.visible(&v.its.p, &light_record.p)
                                                {
                                                    if let PDF::SolidAngle(pdf_bsdf) = v
                                                        .its
                                                        .mesh
                                                        .bsdf
                                                        .pdf(&v.its.uv, &v.its.wi, &d_out_local)
                                                    {
                                                        // FIXME: Need to check the visibility
                                                        (
                                                            v.throughput
                                                                * v.its.mesh.bsdf.eval(
                                                                    &v.its.uv,
                                                                    &v.its.wi,
                                                                    &d_out_local,
                                                                )
                                                                * light_record.weight,
                                                            (light_pdf + pdf_bsdf) * v.pdf_ratio,
                                                        )
                                                    } else {
                                                        (Color::zero(), 0.0)
                                                    }
                                                } else {
                                                    (Color::zero(), 0.0)
                                                }
                                            }
                                            _ => panic!("Not covered case"),
                                        }
                                    })
                                    .collect::<Vec<(Color, f32)>>();

                                for (i, v) in shift_info.into_iter().enumerate() {
                                    let mut pdf_total = base_info.1;
                                    if v.1 > 0.0 {
                                        pdf_total += v.1;
                                    }
                                    let weight = base_pdf / pdf_total;
                                    l_i.main += base_info.0; // * weight;
                                                             //l_i.radiances[i] += v.0 * weight;
                                    l_i.gradients[i] += (v.0 - base_info.0) * weight;
                                }
                            }
                        }
                    }

                    ///////////////////////////////
                    // BSDF Sampling
                    if i == 1 {
                        if v.its.cos_theta() > 0.0 {
                            l_i.very_direct += v.its.mesh.emission * v.throughput;
                        }
                    } else {
                        // If the main light touch a light
                        // we need to do the MIS
                        // FIXME: Assuming non diract functions
                        if v.its.mesh.is_light() && v.its.cos_theta() > 0.0 {
                            let (pred_vertex_pos, pred_vertex_pdf) =
                                match &base_path.vertices[i - 1] {
                                    &Vertex::Surface(ref v) => {
                                        (v.its.p, v.sampled_bsdf.as_ref().unwrap().pdf.value())
                                    }
                                    _ => panic!("Wrong vertex type"),
                                };

                            // FIXME: Assuming non diract functions
                            let light_pdf = scene
                                .direct_pdf(LightSamplingPDF {
                                    mesh: v.its.mesh,
                                    o: pred_vertex_pos,
                                    p: v.its.p,
                                    n: v.its.n_g, // FIXME: Geometrical normal?
                                    dir: base_path.edges[i - 1].d,
                                })
                                .value();

                            let (base_pdf, base_info) = (
                                pred_vertex_pdf,
                                (
                                    v.throughput * (&v.its.mesh.emission),
                                    pred_vertex_pdf + light_pdf,
                                ),
                            );

                            let shift_info = shift_paths.iter().map(|shift_path| {
                                if shift_path.is_none() {
                                    return (Color::zero(), 0.0);
                                }
                                let shift_path = shift_path.as_ref().unwrap();
                                match shift_path.vertices.get(i) {
                                    None => (Color::zero(), 0.0),
                                    Some(&ShiftVertex::Surface(ref v)) => {
                                        if !(v.its.mesh.is_light() && v.its.cos_theta() > 0.0) {
                                            return (Color::zero(), 0.0);
                                        }

                                        let (pred_vertex_pos, pred_vertex_pdf) =
                                            match &shift_path.vertices[i - 1] {
                                                &ShiftVertex::Surface(ref v) => {
                                                    (v.its.p, v.pdf_bsdf.as_ref().unwrap().value())
                                                }
                                                _ => panic!("Wrong vertex type"),
                                            };

                                        let light_pdf = scene
                                            .direct_pdf(LightSamplingPDF {
                                                mesh: v.its.mesh,
                                                o: pred_vertex_pos,
                                                p: v.its.p,
                                                n: v.its.n_g, // FIXME: Geometrical normal?
                                                dir: shift_path.edges[i - 1].d,
                                            })
                                            .value();
                                        (
                                            v.throughput * (&v.its.mesh.emission),
                                            light_pdf + pred_vertex_pdf,
                                        )
                                    }
                                    _ => panic!("Wrong vertex type!"),
                                }
                            });

                            for (i, v) in shift_info.into_iter().enumerate() {
                                let mut pdf_total = base_info.1;
                                if v.1 > 0.0 {
                                    pdf_total += v.1;
                                }
                                let weight = base_pdf / pdf_total;
                                l_i.main += base_info.0; // * weight;
                                                         //l_i.radiances[i] += v.0 * weight;
                                l_i.gradients[i] += (v.0 - base_info.0) * weight;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        l_i
    }
}