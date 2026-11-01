//! The display mesh of a large straight prism (a planar profile extruded along one direction:
//! caps square to it, walls of planes and curves swept along it), made from its rims.
//!
//! The reference mesher projects every boundary point onto its face's surface and the seam
//! check fits every face again, which costs seconds for a drilled board of thousands of holes.
//! Here each rim is sampled once (by the same chord, angle and crowding rules), every wall is a
//! strip between its two rims, and each cap is one constrained triangulation of its loops. The
//! faces, edges and their order are the body's, as from the reference mesher.

use std::collections::HashMap;

use solidcraft_geom::{Mesh, Vec3};
use truck_meshalgo::tessellation::Deflection;
use truck_modeling as mt;

use crate::body::{Body, from_p3};

/// Fewest faces a body needs to be meshed here; smaller bodies keep the reference mesher's
/// triangulation, which their designs were checked against.
pub(crate) const MIN_FACES: usize = 256;
/// Most faces a body may have to be meshed here.
const MAX_FACES: usize = 1 << 20;
/// Most segments one rim is divided into.
const MAX_RIM_SEGMENTS: usize = 1 << 12;
/// Deepest bisection of one rim.
const MAX_DEPTH: u32 = 12;

/// What a face of the prism is.
enum Part {
    /// A planar cap square to the axis.
    Cap,
    /// A wall swept along the axis: its rim at the low level (as its boundary runs) and at the
    /// high level, and its two sides along the axis.
    Wall { low: mt::Edge, high: mt::Edge, curved: bool },
}

/// One wall's low rim as sampled: points in the order the wall's boundary runs and the curve's
/// tangent there.
struct Rim {
    points: Vec<Vec3>,
    tangents: Vec<Vec3>,
    /// The circle the rim lies on (centre, radius), for curved rims that do.
    circle: Option<(Vec3, f64)>,
}

/// The display mesh of `b` at chord tolerance `tol` and facet angle `angle` (as
/// [`Body::reference_display_mesh_angular`] takes them), or `None` when `b` is not a large
/// straight prism this can mesh (the caller meshes it the reference way).
pub(crate) fn display_mesh(b: &Body, tol: f64, angle: f64) -> Option<Mesh> {
    let faces: Vec<&mt::Face> = b.solid.face_iter().collect();
    if !(MIN_FACES..=MAX_FACES).contains(&faces.len()) || b.solid.boundaries().len() != 1 {
        return None;
    }
    let tol = if tol.is_finite() && tol > 0.0 { tol } else { 0.05 };
    let tol = tol.max(b.size() * 1e-6);
    let deflection = if angle.is_finite() && angle > 0.0 {
        Deflection { chord: tol, angle: angle.max(crate::body::MIN_DISPLAY_ANGLE), floor: tol * crate::body::DISPLAY_ANGLE_FLOOR, display: true }
    } else {
        Deflection { display: true, ..Deflection::chord(tol) }
    };
    crate::guard("prism display mesh", || Ok(mesh_prism(b, &faces, deflection))).ok().flatten()
}

fn mesh_prism(b: &Body, faces: &[&mt::Face], deflection: Deflection) -> Option<Mesh> {
    let geo = (b.size() * 1e-7).max(1e-9) * 10.0;
    let (d, parts, low, high) = axis_candidates(faces).into_iter().find_map(|d| classify(faces, d, geo))?;
    let height = high - low;
    // Each rim edge belongs to one wall; every cap edge must be one.
    let mut owner: HashMap<mt::EdgeID, usize> = HashMap::new();
    for (fi, part) in parts.iter().enumerate() {
        if let Part::Wall { low, high, .. } = part {
            for e in [low, high] {
                if owner.insert(e.id(), fi).is_some() {
                    return None;
                }
            }
        }
    }
    // Which caps each wall's rims bound, for the crowding rule.
    let mut caps_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for (fi, (f, part)) in faces.iter().zip(&parts).enumerate() {
        if !matches!(part, Part::Cap) {
            continue;
        }
        for w in f.boundaries() {
            for e in w.edge_iter() {
                let wall = *owner.get(&e.id())?;
                caps_of.entry(wall).or_default().push(fi);
            }
        }
    }
    // Sample the low rims: a cap bounded by more segments than the reference mesher allows
    // gets its curved rims at twice the angle, and again (`Deflection::relaxed`).
    let mut level: Vec<u8> = vec![0; faces.len()];
    let mut rims: Vec<Option<Rim>> = (0..faces.len()).map(|_| None).collect();
    let mut todo: Vec<usize> = (0..faces.len()).filter(|i| matches!(parts.get(*i), Some(Part::Wall { .. }))).collect();
    let mut divisions: Vec<Divisions> = (0..=Deflection::MAX_RELAX).map(|_| Divisions::new()).collect();
    loop {
        for &wi in &todo {
            let Some(Part::Wall { low, curved, .. }) = parts.get(wi) else { return None };
            let l = *level.get(wi)?;
            let rim = sample_rim(low, *curved, &deflection.relaxed(l), d, geo, divisions.get_mut(usize::from(l))?)?;
            *rims.get_mut(wi)? = Some(rim);
        }
        let mut count: Vec<usize> = vec![0; faces.len()];
        for (wall, caps) in &caps_of {
            let n = rims.get(*wall)?.as_ref()?.points.len().saturating_sub(1);
            for c in caps {
                *count.get_mut(*c)? += n;
            }
        }
        todo.clear();
        for (wall, caps) in &caps_of {
            let curved = matches!(parts.get(*wall), Some(Part::Wall { curved: true, .. }));
            let crowded = caps.iter().any(|c| count.get(*c).is_some_and(|n| *n > Deflection::MAX_ANGLE_BOUNDARY));
            let l = level.get_mut(*wall)?;
            if curved && crowded && deflection.angular() && *l < Deflection::MAX_RELAX {
                *l += 1;
                todo.push(*wall);
            }
        }
        if todo.is_empty() {
            break;
        }
        // In face order, so the same rim seeds each shape's division every time.
        todo.sort_unstable();
        todo.dedup();
    }
    // Every edge's polyline, in the edge's own (absolute) direction.
    let mut lines: HashMap<mt::EdgeID, Vec<Vec3>> = HashMap::new();
    for (wi, part) in parts.iter().enumerate() {
        let Part::Wall { low, high, .. } = part else { continue };
        let rim = rims.get(wi)?.as_ref()?;
        let absolute = |e: &mt::Edge, oriented: Vec<Vec3>| if e.orientation() { oriented } else { oriented.into_iter().rev().collect() };
        // The high rim runs the other way round the wall.
        let raised: Vec<Vec3> = rim.points.iter().rev().map(|p| *p + d * height).collect();
        let (front, back) = (from_p3(high.front().point()), from_p3(high.back().point()));
        if raised.first()?.dist(front) > geo * 10.0 || raised.last()?.dist(back) > geo * 10.0 {
            return None;
        }
        lines.insert(low.id(), absolute(low, rim.points.clone()));
        lines.insert(high.id(), absolute(high, raised));
    }
    let (edges, edge_faces) = crate::body::edge_order(faces.iter().copied());
    for e in &edges {
        lines.entry(e.id()).or_insert_with(|| vec![from_p3(e.absolute_front().point()), from_p3(e.absolute_back().point())]);
    }
    let oriented = |e: &mt::Edge| -> Option<Vec<Vec3>> {
        let line = lines.get(&e.id())?;
        Some(if e.orientation() { line.clone() } else { line.iter().rev().copied().collect() })
    };
    // Caps: one constrained triangulation each, on the worker pool.
    let caps: Vec<usize> = (0..faces.len()).filter(|i| matches!(parts.get(*i), Some(Part::Cap))).collect();
    let capped = crate::par::map("prism cap", &caps, caps.len() > 1, |&fi| {
        let f = faces.get(fi).ok_or_else(|| crate::KernelError::Failed("cap".into()))?;
        cap_mesh(f, &oriented).ok_or_else(|| crate::KernelError::Failed("cap triangulation".into()))
    });
    let mut capped = caps.iter().zip(capped).map(|(fi, m)| m.ok().map(|m| (*fi, m))).collect::<Option<HashMap<usize, FaceMesh>>>()?;
    let mut out = Mesh::default();
    for (fi, part) in parts.iter().enumerate() {
        let piece = match part {
            Part::Cap => capped.remove(&fi)?,
            Part::Wall { .. } => wall_mesh(rims.get(fi)?.as_ref()?, d, height),
        };
        let base = u32::try_from(out.positions.len()).ok()?;
        let face = u32::try_from(fi).ok()?;
        out.positions.extend(piece.positions);
        out.normals.extend(piece.normals);
        for t in piece.triangles {
            out.triangles.push([t[0].checked_add(base)?, t[1].checked_add(base)?, t[2].checked_add(base)?]);
            out.tri_face.push(face);
        }
    }
    out.edges = edges.iter().map(oriented).collect::<Option<Vec<_>>>()?;
    out.edge_faces = edge_faces.into_iter().map(|fs| fs.into_iter().map(|f| u32::try_from(f).unwrap_or(u32::MAX)).collect()).collect();
    // A seam joins two pieces of one smooth surface: curved walls meeting without a crease,
    // on the same circle or neither on one (as the reference check fits them).
    out.seams = edges
        .iter()
        .zip(&out.edge_faces)
        .map(|(e, fs)| {
            let [a, c] = fs.as_slice() else { return false };
            let (Some(ra), Some(rc)) = (rims.get(*a as usize).and_then(Option::as_ref), rims.get(*c as usize).and_then(Option::as_ref)) else {
                return false;
            };
            let curved = |i: u32| matches!(parts.get(i as usize), Some(Part::Wall { curved: true, .. }));
            curved(*a) && curved(*c) && smooth_join(e, ra, rc, d) && same_circle(ra.circle, rc.circle, geo * 100.0)
        })
        .collect();
    (out.smooth_edges, out.sharp_edges) = crate::smooth_edges::flags(b, &out);
    Some(out)
}

/// Plane normals to try as the axis: those of the faces with the most edges (caps) first.
fn axis_candidates(faces: &[&mt::Face]) -> Vec<Vec3> {
    let mut planes: Vec<(usize, Vec3)> = faces
        .iter()
        .filter_map(|f| match f.oriented_surface() {
            mt::Surface::Plane(p) => {
                let n = p.normal();
                Some((f.boundaries().iter().map(|w| w.len()).sum(), Vec3::new(n.x, n.y, n.z).normalized()?))
            }
            _ => None,
        })
        .collect();
    planes.sort_by_key(|p| std::cmp::Reverse(p.0));
    let mut out: Vec<Vec3> = Vec::new();
    for (_, n) in planes {
        if out.len() >= 3 {
            break;
        }
        if !out.iter().any(|m| m.dot(n).abs() > 1.0 - 1e-9) {
            out.push(n);
        }
    }
    out
}

/// Every face of the prism along `d` and the two levels of its caps, or `None` when it is not
/// one: caps on two levels, each wall one loop of a low rim, a high rim and two sides along `d`.
fn classify(faces: &[&mt::Face], d: Vec3, geo: f64) -> Option<(Vec3, Vec<Part>, f64, f64)> {
    let mut levels: Vec<f64> = Vec::new();
    let mut walls: Vec<Option<(mt::Edge, mt::Edge, bool)>> = Vec::with_capacity(faces.len());
    let mut work = faces.len().saturating_mul(256);
    for f in faces {
        let surface = f.oriented_surface();
        let curved = match &surface {
            mt::Surface::Plane(p) => {
                let n = p.normal();
                let n = Vec3::new(n.x, n.y, n.z).normalized()?;
                if n.dot(d).abs() > 1.0 - 1e-9 {
                    let h = from_p3(p.origin()).dot(d);
                    if !levels.iter().any(|l| (l - h).abs() < geo) {
                        levels.push(h);
                    }
                    walls.push(None);
                    continue;
                }
                if n.dot(d).abs() > 1e-9 {
                    return None;
                }
                false
            }
            other => {
                if !crate::prism::wall_along(other, d, geo, &mut work) {
                    return None;
                }
                true
            }
        };
        let [wire] = f.boundaries().try_into().ok()?;
        if wire.len() != 4 {
            return None;
        }
        let edges: Vec<mt::Edge> = wire.edge_iter().cloned().collect();
        // The sides are lines along d; the rims lie between them.
        let side =
            |e: &mt::Edge| matches!(e.curve(), mt::Curve::Line(_)) && (from_p3(e.back().point()) - from_p3(e.front().point())).cross(d).len() < geo;
        let start = (0..4).find(|i| edges.get(*i).is_some_and(|e| !side(e)) && edges.get((i + 1) % 4).is_some_and(side))?;
        let [r0, s0, r1, s1] = [0, 1, 2, 3].map(|k| edges.get((start + k) % 4).cloned());
        let (r0, s0, r1, s1) = (r0?, s0?, r1?, s1?);
        if side(&r0) || side(&r1) || !side(&s0) || !side(&s1) {
            return None;
        }
        // Going up the side after the low rim.
        let rise = (from_p3(s0.back().point()) - from_p3(s0.front().point())).dot(d);
        let (lo, hi) = if rise > 0.0 { (r0, r1) } else { (r1, r0) };
        walls.push(Some((lo, hi, curved)));
    }
    let [a, c] = levels.as_slice() else { return None };
    let (low, high) = if a < c { (*a, *c) } else { (*c, *a) };
    if high - low < geo * 10.0 {
        return None;
    }
    // Rims on their levels: a low rim's ends at the low level, its loop's high rim's at the high.
    let at = |e: &mt::Edge, h: f64| [e.front(), e.back()].iter().all(|v| (from_p3(v.point()).dot(d) - h).abs() < geo * 10.0);
    let mut parts = Vec::with_capacity(faces.len());
    for w in walls {
        parts.push(match w {
            None => Part::Cap,
            Some((lo, hi, curved)) => {
                if !at(&lo, low) || !at(&hi, high) {
                    return None;
                }
                Part::Wall { low: lo, high: hi, curved }
            }
        });
    }
    Some((d, parts, low, high))
}

/// A curve's division in its own direction, relative to its first control point: the same for
/// every copy of one curve moved without turning (the holes of one drill), so each shape is
/// divided once.
struct Division {
    points: Vec<Vec3>,
    tangents: Vec<Vec3>,
    circle: Option<(Vec3, f64)>,
}

/// Divisions by curve shape (see [`shape_key`]).
type Divisions = HashMap<Vec<u64>, std::rc::Rc<Division>>;

/// A key for a B-spline or NURBS curve's shape up to translation: its degree, knots, weights and
/// control points less the first (rounded to `q`), and the first point it was taken from.
fn shape_key(c: &mt::Curve, q: f64) -> Option<(Vec<u64>, Vec3)> {
    let mut key: Vec<u64> = Vec::new();
    let push_points = |key: &mut Vec<u64>, points: &mut dyn Iterator<Item = (Vec3, f64)>| -> Option<Vec3> {
        let (first, w0) = points.next()?;
        key.push(w0.to_bits());
        for (p, w) in points {
            let r = p - first;
            for x in [r.x, r.y, r.z] {
                let k = (x / q).round();
                if !k.is_finite() || k.abs() > 1e15 {
                    return None;
                }
                key.push((k as i64) as u64);
            }
            key.push(w.to_bits());
        }
        Some(first)
    };
    let first = match c {
        mt::Curve::BSplineCurve(c) => {
            key.push(1);
            key.extend(c.knot_vec().iter().map(|k| k.to_bits()));
            push_points(&mut key, &mut c.control_points().iter().map(|p| (from_p3(*p), 1.0)))?
        }
        mt::Curve::NurbsCurve(c) => {
            key.push(2);
            key.extend(c.knot_vec().iter().map(|k| k.to_bits()));
            let mut points = c.control_points().iter().map(|p| (Vec3::new(p.x / p.w, p.y / p.w, p.z / p.w), p.w));
            push_points(&mut key, &mut points)?
        }
        _ => return None,
    };
    let finite = [first.x, first.y, first.z].iter().all(|x| x.is_finite());
    finite.then_some((key, first))
}

/// A wall's low rim divided as the reference mesher divides an edge: until no segment sags
/// more than the chord tolerance, nor turns more than the angle while it sags more than the
/// floor (`Deflection::turns`); a curved rim gets at least two segments when it turns more than
/// a quarter. Points run the way the wall's boundary runs.
fn sample_rim(e: &mt::Edge, curved: bool, deflection: &Deflection, d: Vec3, geo: f64, divisions: &mut Divisions) -> Option<Rim> {
    let (front, back) = (from_p3(e.front().point()), from_p3(e.back().point()));
    if !curved {
        let t = back - front;
        return Some(Rim { points: vec![front, back], tangents: vec![t, t], circle: None });
    }
    // Divided in the curve's own direction, then turned the way the boundary runs.
    let c = e.curve();
    let (division, at) = match shape_key(&c, geo * 1e-3) {
        Some((key, first)) => match divisions.get(&key) {
            Some(division) => (division.clone(), first),
            None => {
                let division = std::rc::Rc::new(divide(&c, deflection, d, geo, first)?);
                divisions.insert(key, division.clone());
                (division, first)
            }
        },
        None => (std::rc::Rc::new(divide(&c, deflection, d, geo, Vec3::ZERO)?), Vec3::ZERO),
    };
    let mut points: Vec<Vec3> = division.points.iter().map(|p| *p + at).collect();
    let mut tangents = division.tangents.clone();
    if !e.orientation() {
        points.reverse();
        tangents.reverse();
        tangents.iter_mut().for_each(|t| *t = -*t);
    }
    // The ends are the edge's vertices exactly, as the caps and sides have them.
    if points.first()?.dist(front) > geo * 10.0 || points.last()?.dist(back) > geo * 10.0 {
        return None;
    }
    *points.first_mut()? = front;
    *points.last_mut()? = back;
    Some(Rim { points, tangents, circle: division.circle.map(|(o, r)| (o + at, r)) })
}

/// `c` divided over its range, points less `origin`.
fn divide(c: &mt::Curve, deflection: &Deflection, d: Vec3, geo: f64, origin: Vec3) -> Option<Division> {
    use mt::{BoundedCurve, ParametricCurve};
    let (t0, t1) = c.range_tuple();
    if !(t0.is_finite() && t1.is_finite() && t1 > t0) {
        return None;
    }
    let point = |t: f64| from_p3(c.subs(t));
    let tangent = |t: f64| {
        let v = c.der(t);
        Vec3::new(v.x, v.y, v.z)
    };
    let turn2 = deflection.turn2();
    let split = |a: f64, b: f64, depth: u32| {
        if depth >= MAX_DEPTH {
            return false;
        }
        let (pa, pb, pm) = (point(a), point(b), point((a + b) * 0.5));
        let sag = pm.dist((pa + pb) * 0.5);
        if sag > deflection.chord || deflection.turns(turn2, sag * sag, pa.dist(pb).powi(2)) {
            return true;
        }
        // A rim's one segment never turns more than a quarter (a half circle is no line).
        depth == 0 && tangent(a).normalized().zip(tangent(b).normalized()).is_some_and(|(u, v)| u.dot(v) < 0.0)
    };
    let mut params = vec![t0];
    let mut stack = vec![(t0, t1, 0u32)];
    while let Some((a, b, depth)) = stack.pop() {
        if params.len() > MAX_RIM_SEGMENTS {
            return None;
        }
        if split(a, b, depth) {
            let m = (a + b) * 0.5;
            stack.push((m, b, depth + 1));
            stack.push((a, m, depth + 1));
        } else {
            params.push(b);
        }
    }
    let points = params.iter().map(|t| point(*t) - origin).collect();
    let tangents = params.iter().map(|t| tangent(*t)).collect();
    let circle = circle_of(&point, t0, t1, d, geo).map(|(o, r)| (o - origin, r));
    Some(Division { points, tangents, circle })
}

/// The circle a curve in a plane square to `d` lies on: through its ends and middle, checked
/// at two more points.
fn circle_of(point: &dyn Fn(f64) -> Vec3, t0: f64, t1: f64, d: Vec3, geo: f64) -> Option<(Vec3, f64)> {
    let at = |k: f64| point(t0 + (t1 - t0) * k);
    let (a, b, c) = (at(0.0), at(0.5), at(1.0));
    // Circumcentre of a, b, c in their plane.
    let (ab, ac) = (b - a, c - a);
    let n = ab.cross(ac);
    let nn = n.dot(n);
    if nn < geo * geo * geo * geo || n.normalized()?.cross(d).len() > 1e-6 {
        return None;
    }
    let centre = a + (n.cross(ab) * ac.dot(ac) + ac.cross(n) * ab.dot(ab)) * (0.5 / nn);
    let r = centre.dist(a);
    [0.25, 0.75].iter().all(|k| (at(*k).dist(centre) - r).abs() < geo * 100.0).then_some((centre, r))
}

/// Two walls meet without a crease along their shared side `e` (their rims' tangents agree
/// where the side stands).
fn smooth_join(e: &mt::Edge, a: &Rim, c: &Rim, d: Vec3) -> bool {
    let p = from_p3(e.absolute_front().point());
    // How far from the side, seen along the axis.
    let off = |q: &Vec3| {
        let w = *q - p;
        (w - d * w.dot(d)).len()
    };
    let tangent_at = |r: &Rim| -> Option<Vec3> {
        let (first, last) = (r.points.first()?, r.points.last()?);
        let t = if off(first) < off(last) { r.tangents.first()? } else { r.tangents.last()? };
        t.normalized()
    };
    let (Some(ta), Some(tc)) = (tangent_at(a), tangent_at(c)) else { return false };
    // Both rims run the same way round the loop: one ends where the other starts.
    ta.dot(tc) > 1.0 - 2e-3
}

fn same_circle(a: Option<(Vec3, f64)>, c: Option<(Vec3, f64)>, tol: f64) -> bool {
    match (a, c) {
        (Some((ca, ra)), Some((cc, rc))) => ca.dist(cc) < tol && (ra - rc).abs() < tol,
        (None, None) => true,
        _ => false,
    }
}

/// One face's triangles, indexed into its own vertices.
struct FaceMesh {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
}

/// A wall: the strip between its low rim and that rim raised to the high level, its normal the
/// rim's tangent across the axis (outward: the boundary runs along the low rim, then up).
fn wall_mesh(rim: &Rim, d: Vec3, height: f64) -> FaceMesh {
    let n = rim.points.len();
    let mut positions = Vec::with_capacity(2 * n);
    let mut normals = Vec::with_capacity(2 * n);
    for (p, t) in rim.points.iter().zip(&rim.tangents) {
        let normal = t.cross(d).normalized().unwrap_or(Vec3::Z);
        positions.push(*p);
        positions.push(*p + d * height);
        normals.push(normal);
        normals.push(normal);
    }
    let mut triangles = Vec::with_capacity(2 * n);
    for i in 0..n.saturating_sub(1) {
        let Ok(k) = u32::try_from(2 * i) else { break };
        let (b0, t0, b1, t1) = (k, k + 1, k + 2, k + 3);
        triangles.push([b0, b1, t1]);
        triangles.push([b0, t1, t0]);
    }
    FaceMesh { positions, normals, triangles }
}

/// A cap: its loops, as their edges' polylines run round it, triangulated in its plane.
fn cap_mesh(f: &mt::Face, oriented: &dyn Fn(&mt::Edge) -> Option<Vec<Vec3>>) -> Option<FaceMesh> {
    let mt::Surface::Plane(plane) = f.oriented_surface() else { return None };
    let n = plane.normal();
    let normal = Vec3::new(n.x, n.y, n.z).normalized()?;
    let u = normal.any_perp().normalized()?;
    let v = normal.cross(u);
    let mut positions: Vec<Vec3> = Vec::new();
    let mut loops: Vec<std::ops::Range<usize>> = Vec::new();
    for w in f.boundaries() {
        let start = positions.len();
        for e in w.edge_iter() {
            let line = oriented(e)?;
            positions.extend(line.iter().take(line.len().saturating_sub(1)));
        }
        if positions.len() - start < 3 {
            return None;
        }
        loops.push(start..positions.len());
    }
    let uv: Vec<[f64; 2]> = positions.iter().map(|p| [p.dot(u), p.dot(v)]).collect();
    let triangles = crate::tess::cdt::triangulate_loops(&uv, &loops)?;
    let normals = vec![normal; positions.len()];
    Some(FaceMesh { positions, normals, triangles })
}
