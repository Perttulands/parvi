//! A small CPU preview for self-contained GLB models.
use gpui::RenderImage;
use std::sync::Arc;

#[derive(Clone)]
pub struct Model {
    triangles: Vec<Triangle>,
    center: [f32; 3],
    radius: f32,
}
#[derive(Clone)]
struct Triangle {
    p: [[f32; 3]; 3],
    color: [f32; 4],
}
#[derive(Clone)]
pub struct Preview {
    model: Arc<Model>,
    pub image: Arc<RenderImage>,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub last: Option<(f32, f32)>,
}
const SIDE: u32 = 512;
const MAX_DEPTH: usize = 64;
const MAX_TRIANGLES: usize = 2_000_000;
const MAX_VISITS: usize = 100_000;
impl Model {
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| e.to_string())?;
        let blob = gltf.blob.as_deref().ok_or("GLB has no binary buffer")?;
        let scene = gltf
            .default_scene()
            .or_else(|| gltf.scenes().next())
            .ok_or("Model has no scene")?;
        let mut triangles = Vec::new();
        fn visit(
            node: gltf::Node<'_>,
            parent: [[f32; 4]; 4],
            blob: &[u8],
            out: &mut Vec<Triangle>,
            depth: usize,
            visits: &mut usize,
        ) -> Result<(), String> {
            // A node graph with a cycle, or a subtree shared many times over,
            // would otherwise recurse without end or never finish.
            *visits += 1;
            if depth > MAX_DEPTH || *visits > MAX_VISITS {
                return Err("Model's node tree is too deep or too large".into());
            }
            let local = node.transform().matrix();
            let mut matrix = [[0.; 4]; 4];
            for (c, column) in matrix.iter_mut().enumerate() {
                for (r, value) in column.iter_mut().enumerate() {
                    *value = (0..4).map(|k| parent[k][r] * local[c][k]).sum();
                }
            }
            if let Some(mesh) = node.mesh() {
                for primitive in mesh.primitives() {
                    if primitive.mode() != gltf::mesh::Mode::Triangles {
                        continue;
                    }
                    let reader = primitive.reader(|b| match b.source() {
                        gltf::buffer::Source::Bin => Some(blob),
                        _ => None,
                    });
                    let positions: Vec<_> = reader
                        .read_positions()
                        .ok_or("Missing positions or external buffer")?
                        .map(|p| {
                            let v = [p[0], p[1], p[2], 1.];
                            std::array::from_fn(|r| {
                                (0..4).map(|c| matrix[c][r] * v[c]).sum::<f32>()
                            })
                        })
                        .collect();
                    let indices: Vec<u32> = reader
                        .read_indices()
                        .map(|v| v.into_u32().collect())
                        .unwrap_or_else(|| (0..positions.len() as u32).collect());
                    let color = primitive
                        .material()
                        .pbr_metallic_roughness()
                        .base_color_factor();
                    for ix in indices.chunks_exact(3) {
                        let [a, b, c] = [ix[0] as usize, ix[1] as usize, ix[2] as usize];
                        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
                            return Err("Invalid triangle indices".into());
                        }
                        let p = [positions[a], positions[b], positions[c]];
                        if p.iter().flatten().any(|n| !n.is_finite()) {
                            return Err("Invalid vertex coordinates".into());
                        }
                        if out.len() >= MAX_TRIANGLES {
                            return Err(format!("Model has more than {MAX_TRIANGLES} triangles"));
                        }
                        out.push(Triangle { p, color });
                    }
                }
            }
            for child in node.children() {
                visit(child, matrix, blob, out, depth + 1, visits)?;
            }
            Ok(())
        }
        let identity = [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ];
        let mut visits = 0;
        for node in scene.nodes() {
            visit(node, identity, blob, &mut triangles, 0, &mut visits)?;
        }
        if triangles.is_empty() {
            return Err("Model contains no triangles".into());
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in triangles.iter().flat_map(|t| t.p) {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        let center = std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5);
        let radius = triangles
            .iter()
            .flat_map(|t| t.p)
            .map(|p| ((0..3).map(|i| (p[i] - center[i]).powi(2)).sum::<f32>()).sqrt())
            .fold(0., f32::max)
            .max(0.00001);
        Ok(Self {
            triangles,
            center,
            radius,
        })
    }
    fn render(&self, yaw: f32, pitch: f32, zoom: f32) -> Arc<RenderImage> {
        let mut data = vec![0u8; (SIDE * SIDE * 4) as usize];
        let mut depth = vec![f32::NEG_INFINITY; (SIDE * SIDE) as usize];
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let project = |p: [f32; 3]| {
            let x = p[0] - self.center[0];
            let y = p[1] - self.center[1];
            let z = p[2] - self.center[2];
            let a = cy * x - sy * z;
            let b = sy * x + cy * z;
            let v = cp * y - sp * b;
            let d = sp * y + cp * b;
            let scale = SIDE as f32 * 0.42 * zoom / self.radius;
            [
                SIDE as f32 / 2. + a * scale,
                SIDE as f32 / 2. - v * scale,
                d * scale,
            ]
        };
        for tri in &self.triangles {
            let p = tri.p.map(project);
            let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
                (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
            };
            let area = edge(p[0], p[1], p[2][0], p[2][1]);
            if area.abs() < 0.001 {
                continue;
            }
            let u = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
            let v = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-10);
            let light =
                (0.35 + 0.65 * ((n[0] * 0.3 - n[1] * 0.5 + n[2] * 0.8) / len).abs()).min(1.);
            let minx = p
                .iter()
                .map(|v| v[0])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .clamp(0., (SIDE - 1) as f32) as u32;
            let maxx = p
                .iter()
                .map(|v| v[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .clamp(0., (SIDE - 1) as f32) as u32;
            let miny = p
                .iter()
                .map(|v| v[1])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .clamp(0., (SIDE - 1) as f32) as u32;
            let maxy = p
                .iter()
                .map(|v| v[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .clamp(0., (SIDE - 1) as f32) as u32;
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let (xv, yv) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = edge(p[1], p[2], xv, yv) / area;
                    let w1 = edge(p[2], p[0], xv, yv) / area;
                    let w2 = 1. - w0 - w1;
                    if w0 < 0. || w1 < 0. || w2 < 0. {
                        continue;
                    }
                    let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                    let i = (y * SIDE + x) as usize;
                    if z > depth[i] {
                        depth[i] = z;
                        for c in 0..3 {
                            data[i * 4 + c] = (tri.color[2 - c].clamp(0., 1.) * light * 255.) as u8;
                        }
                        data[i * 4 + 3] = 255;
                    }
                }
            }
        }
        let buffer = image::RgbaImage::from_raw(SIDE, SIDE, data).expect("fixed image dimensions");
        Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
    }
}
impl Preview {
    pub fn new(model: Model) -> Self {
        let model = Arc::new(model);
        let image = model.render(0.7, 0.4, 1.);
        Self {
            model,
            image,
            yaw: 0.7,
            pitch: 0.4,
            zoom: 1.,
            last: None,
        }
    }
    pub fn redraw(&mut self) {
        self.image = self.model.render(self.yaw, self.pitch, self.zoom);
    }
    pub fn reset(&mut self) {
        self.yaw = 0.7;
        self.pitch = 0.4;
        self.zoom = 1.;
        self.redraw();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        glb(br#"{"mesh":0}"#)
    }
    fn glb(node: &[u8]) -> Vec<u8> {
        let mut bin = Vec::new();
        for v in [-1f32, -1., 0., 1., -1., 0., 0., 1., 0.] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        let mut json =
            br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":["#.to_vec();
        json.extend_from_slice(node);
        json.extend_from_slice(br#"],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"buffers":[{"byteLength":36}],"bufferViews":[{"buffer":0,"byteLength":36}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[-1,-1,0],"max":[1,1,0]}]}"#);
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        glb
    }
    #[test]
    fn load_fit_and_draw() {
        let m = Model::load(&fixture()).unwrap();
        assert_eq!(m.triangles.len(), 1);
        assert_eq!(m.center, [0., 0., 0.]);
        assert!((m.radius - 2f32.sqrt()).abs() < 0.001);
        let image = m.render(0., 0., 1.);
        let bytes = image.as_bytes(0).unwrap();
        assert!(bytes.iter().skip(3).step_by(4).filter(|&&a| a > 0).count() > 1000);
        assert_eq!(bytes[((SIDE * SIDE / 2 + SIDE / 2) * 4 + 3) as usize], 255);
    }
    #[test]
    fn depth_and_reset() {
        let triangle = |z, color| Triangle {
            p: [[-1., -1., z], [1., -1., z], [0., 1., z]],
            color,
        };
        let model = Model {
            triangles: vec![
                triangle(0.5, [1., 0., 0., 1.]),
                triangle(-0.5, [0., 0., 1., 1.]),
            ],
            center: [0.; 3],
            radius: 2.,
        };
        let image = model.render(0., 0., 1.);
        let data = image.as_bytes(0).unwrap();
        let center = ((SIDE * SIDE / 2 + SIDE / 2) * 4) as usize;
        assert!(
            data[center + 2] > 0,
            "front red triangle must occlude the blue one"
        );
        assert_eq!(data[center], 0);
        let mut preview = Preview::new(model);
        preview.yaw = 2.;
        preview.pitch = -1.;
        preview.zoom = 3.;
        preview.reset();
        assert_eq!((preview.yaw, preview.pitch, preview.zoom), (0.7, 0.4, 1.));
    }
    #[test]
    fn rejects_bad_models() {
        assert!(Model::load(b"glTF").is_err());
    }
    #[test]
    fn a_node_that_contains_itself_is_refused_not_followed() {
        let error = Model::load(&glb(br#"{"mesh":0,"children":[0]}"#)).err();
        assert_eq!(
            error.as_deref(),
            Some("Model's node tree is too deep or too large")
        );
    }
    #[test]
    fn a_subtree_shared_many_times_over_is_refused_not_walked() {
        // 40 levels, each node listing the next twice: 2^40 visits if walked.
        let nodes: Vec<String> = (0..40)
            .map(|i| format!(r#"{{"children":[{0},{0}]}}"#, i + 1))
            .chain([r#"{"mesh":0}"#.to_string()])
            .collect();
        let error = Model::load(&glb(nodes.join(",").as_bytes())).err();
        assert_eq!(
            error.as_deref(),
            Some("Model's node tree is too deep or too large")
        );
    }
}
