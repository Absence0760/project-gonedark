extends Node
# ═══════════════════════════════════════════════════════════════════════════════
#  GOING DARK — Phase 0 control prototype  (THROWAWAY)
# ───────────────────────────────────────────────────────────────────────────────
#  Sole purpose (roadmap Phase 0): prove the embody <-> command loop FEELS GOOD in
#  hand on a touchscreen, before any systems are built. This is disposable Godot —
#  the real engine is Rust/wgpu (decision D10), built fresh in Phase 1. Do not grow
#  this into the game.
#
#  Faithful to the locked design even though it's throwaway:
#   * ONE controllable unit (Phase 0 scope).
#   * Embodiment = swap THIS entity's input source (orders -> live player) + flip
#     vision to avatar-only. No separate character, no respawn.            (D6/D7, inv #5)
#   * "World goes dark": command map is gone while embodied; vignette + a constant
#     "BLIND" tell; alerts-not-intel (a directional flash + haptic, no map reveal). (§6)
#   * Unit is a literal executor: an ordered move just walks straight there.   (§8)
#   * Sim/render split etc. are NOT modelled — this prototype is about FEEL only.
# ═══════════════════════════════════════════════════════════════════════════════

# ── Tunables (expect to twiddle these on-device — that IS the Phase 0 job) ──
const FIELD_SIZE := 40.0          # square field, metres
const UNIT_SPEED := 6.0           # m/s when executing a command-layer move order
const FPS_SPEED := 4.5            # m/s embodied walk
const EYE_HEIGHT := 1.6
const LOOK_SENS_DEG := 0.22       # degrees of look per pixel dragged
const JOY_RADIUS := 130.0         # px, virtual-stick travel
const FIRE_RANGE := 30.0          # m
const FIRE_CONE_DEG := 6.0        # aim assist half-angle
const ALERT_PERIOD := 7.0         # s between faked "something's wrong back home" pings
const ZOOM_MIN := 0.5
const ZOOM_MAX := 3.0

enum Mode { COMMAND, EMBODIED }
var mode: int = Mode.COMMAND

# ── The one entity (shared across both layers — embody only swaps its input) ──
var unit_pos := Vector2(20, 20)   # field metres
var unit_yaw := 0.0               # radians; look/heading yaw
var look_pitch := 0.0             # degrees, clamped
var face_vec := Vector2(0, -1)    # normalised heading in field space (for the map arrow & aim)
var move_target = null            # Vector2 last-order destination, or null

# ── Level ──
var cover: Array = []             # Array[Rect2] in field metres
var enemies: Array = []           # Array of { pos:Vector2, alive:bool, mesh:Node3D, dot:Polygon2D }

# ── Command-layer camera (the MapRoot transform carries pan + zoom) ──
var base_scale := 18.0            # px per metre at zoom 1 (recomputed from viewport)

# ── 3D nodes ──
var world: Node3D
var cam3d: Camera3D

# ── 2D layers ──
var map_layer: CanvasLayer
var map_root: Node2D
var unit_arrow: Polygon2D
var target_marker: Node2D
var fx_layer: CanvasLayer
var vignette: ColorRect
var crosshair: Polygon2D
var joy_base: Polygon2D
var joy_knob: Polygon2D
var blind_label: Label
var alert_label: Label
var alert_flash: ColorRect
var ui_layer: CanvasLayer
var embody_btn: Button
var surface_btn: Button
var fire_btn: Button
var hint_label: Label

# ── Touch tracking ──
var touches: Dictionary = {}      # index -> { pos, start, moved }
var joy_index := -1
var look_index := -1
var joy_vec := Vector2.ZERO
var ui_rects: Array = []          # global Rect2s that must not start a stick/look

# ── Alert state ──
var alert_timer := 0.0
var alert_dir_index := 0
var alert_show := 0.0             # seconds remaining to show the alert banner
const ALERT_DIRS := ["NORTH camp", "EAST camp", "the supply line", "SOUTH flank"]


func _ready() -> void:
	randomize()
	_build_level_data()
	_build_world_3d()
	_build_command_map()
	_build_fx_overlay()
	_build_ui()
	get_viewport().size_changed.connect(_on_resize)
	_on_resize()
	_apply_mode()


# ───────────────────────────────── level ─────────────────────────────────
func _build_level_data() -> void:
	cover = [
		Rect2(8, 8, 4, 4),
		Rect2(25, 9, 6, 3),
		Rect2(14, 23, 3, 9),
		Rect2(27, 26, 5, 5),
		Rect2(18, 14, 3, 3),
	]
	for p in [Vector2(12, 30), Vector2(31, 13), Vector2(24, 29), Vector2(9, 18)]:
		enemies.append({ "pos": p, "alive": true, "mesh": null, "dot": null })


# ───────────────────────────────── 3D world ──────────────────────────────
func _build_world_3d() -> void:
	world = Node3D.new()
	add_child(world)

	var env := WorldEnvironment.new()
	var e := Environment.new()
	e.background_mode = Environment.BG_COLOR
	e.background_color = Color(0.45, 0.58, 0.72)
	e.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	e.ambient_light_color = Color(0.55, 0.58, 0.62)
	e.ambient_light_energy = 1.0
	env.environment = e
	world.add_child(env)

	var sun := DirectionalLight3D.new()
	sun.rotation = Vector3(deg_to_rad(-55), deg_to_rad(40), 0)
	sun.light_energy = 1.1
	world.add_child(sun)

	var ground := MeshInstance3D.new()
	var pm := PlaneMesh.new()
	pm.size = Vector2(FIELD_SIZE, FIELD_SIZE)
	ground.mesh = pm
	ground.position = Vector3(FIELD_SIZE * 0.5, 0, FIELD_SIZE * 0.5)
	ground.material_override = _mat(Color(0.30, 0.40, 0.26))
	world.add_child(ground)

	for r in cover:
		var b := MeshInstance3D.new()
		var bm := BoxMesh.new()
		bm.size = Vector3(r.size.x, 2.6, r.size.y)
		b.mesh = bm
		b.position = Vector3(r.position.x + r.size.x * 0.5, 1.3, r.position.y + r.size.y * 0.5)
		b.material_override = _mat(Color(0.50, 0.48, 0.45))
		world.add_child(b)

	for e_data in enemies:
		var m := MeshInstance3D.new()
		var cm := CapsuleMesh.new()
		cm.radius = 0.45
		cm.height = 1.9
		m.mesh = cm
		m.position = Vector3(e_data.pos.x, 0.95, e_data.pos.y)
		m.material_override = _mat(Color(0.80, 0.18, 0.16))
		world.add_child(m)
		e_data.mesh = m

	cam3d = Camera3D.new()
	cam3d.fov = 75.0
	cam3d.near = 0.05
	world.add_child(cam3d)


func _mat(c: Color) -> StandardMaterial3D:
	var m := StandardMaterial3D.new()
	m.albedo_color = c
	m.roughness = 0.9
	return m


# ───────────────────────────────── command map (top-down 2D) ─────────────
func _build_command_map() -> void:
	map_layer = CanvasLayer.new()
	map_layer.layer = 1
	add_child(map_layer)

	# opaque field backdrop so the 3D world is fully hidden in command view
	var bg := ColorRect.new()
	bg.color = Color(0.08, 0.10, 0.09)
	bg.anchor_right = 1.0
	bg.anchor_bottom = 1.0
	bg.mouse_filter = Control.MOUSE_FILTER_IGNORE
	map_layer.add_child(bg)

	# everything below lives in field*base_scale coords; pan/zoom = MapRoot transform
	map_root = Node2D.new()
	map_layer.add_child(map_root)

	# field outline
	var border := Line2D.new()
	border.width = 2.0
	border.default_color = Color(0.25, 0.55, 0.35)
	border.closed = true
	for c in [Vector2(0, 0), Vector2(FIELD_SIZE, 0), Vector2(FIELD_SIZE, FIELD_SIZE), Vector2(0, FIELD_SIZE)]:
		border.add_point(c * base_scale)
	map_root.add_child(border)

	# grid
	var grid := Line2D.new()
	grid.width = 1.0
	grid.default_color = Color(0.18, 0.30, 0.22, 0.6)
	# (grid drawn as many short segments via a single Line2D would connect them;
	#  use separate Line2D per line instead)
	for i in range(1, int(FIELD_SIZE / 5)):
		var v := Line2D.new()
		v.width = 1.0
		v.default_color = Color(0.16, 0.26, 0.20)
		v.add_point(Vector2(i * 5, 0) * base_scale)
		v.add_point(Vector2(i * 5, FIELD_SIZE) * base_scale)
		map_root.add_child(v)
		var h := Line2D.new()
		h.width = 1.0
		h.default_color = Color(0.16, 0.26, 0.20)
		h.add_point(Vector2(0, i * 5) * base_scale)
		h.add_point(Vector2(FIELD_SIZE, i * 5) * base_scale)
		map_root.add_child(h)

	for r in cover:
		var box := ColorRect.new()
		box.color = Color(0.40, 0.39, 0.36)
		box.position = r.position * base_scale
		box.size = r.size * base_scale
		box.mouse_filter = Control.MOUSE_FILTER_IGNORE
		map_root.add_child(box)

	for e_data in enemies:
		var dot := _circle(0.7 * base_scale, Color(0.85, 0.20, 0.18))
		dot.position = e_data.pos * base_scale
		map_root.add_child(dot)
		e_data.dot = dot

	# move-order marker
	target_marker = _circle(0.5 * base_scale, Color(0.95, 0.85, 0.25, 0.9))
	target_marker.visible = false
	map_root.add_child(target_marker)

	# the unit — an arrow pointing along face_vec
	unit_arrow = Polygon2D.new()
	var s := base_scale
	unit_arrow.polygon = PackedVector2Array([
		Vector2(1.1 * s, 0), Vector2(-0.7 * s, 0.7 * s), Vector2(-0.3 * s, 0), Vector2(-0.7 * s, -0.7 * s)
	])
	unit_arrow.color = Color(0.35, 0.75, 1.0)
	map_root.add_child(unit_arrow)


# ───────────────────────────────── embodied FX overlay ───────────────────
func _build_fx_overlay() -> void:
	fx_layer = CanvasLayer.new()
	fx_layer.layer = 2
	add_child(fx_layer)

	# vignette — the constant, visceral "you are blind" clamp (§6)
	vignette = ColorRect.new()
	vignette.anchor_right = 1.0
	vignette.anchor_bottom = 1.0
	vignette.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var sh := Shader.new()
	sh.code = """
shader_type canvas_item;
void fragment() {
	float d = distance(UV, vec2(0.5));
	float dark = smoothstep(0.30, 0.85, d);
	COLOR = vec4(0.0, 0.0, 0.0, dark * 0.85);
}
"""
	var mat := ShaderMaterial.new()
	mat.shader = sh
	vignette.material = mat
	fx_layer.add_child(vignette)

	# crosshair
	crosshair = Polygon2D.new()
	crosshair.color = Color(1, 1, 1, 0.7)
	fx_layer.add_child(crosshair)

	# virtual stick (drawn where the left thumb lands)
	joy_base = _circle(JOY_RADIUS, Color(1, 1, 1, 0.10))
	joy_base.visible = false
	fx_layer.add_child(joy_base)
	joy_knob = _circle(JOY_RADIUS * 0.42, Color(1, 1, 1, 0.30))
	joy_knob.visible = false
	fx_layer.add_child(joy_knob)

	# directional alert flash (alerts-not-intel: you get a DIRECTION, not the map) (§6)
	alert_flash = ColorRect.new()
	alert_flash.color = Color(1.0, 0.35, 0.15, 0.0)
	alert_flash.mouse_filter = Control.MOUSE_FILTER_IGNORE
	fx_layer.add_child(alert_flash)

	blind_label = Label.new()
	blind_label.text = "●  BLIND — strategic map dark"
	blind_label.add_theme_color_override("font_color", Color(1.0, 0.55, 0.45))
	blind_label.add_theme_font_size_override("font_size", 22)
	fx_layer.add_child(blind_label)

	alert_label = Label.new()
	alert_label.add_theme_color_override("font_color", Color(1.0, 0.8, 0.4))
	alert_label.add_theme_font_size_override("font_size", 30)
	alert_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	alert_label.visible = false
	fx_layer.add_child(alert_label)


# ───────────────────────────────── buttons / hints ───────────────────────
func _build_ui() -> void:
	ui_layer = CanvasLayer.new()
	ui_layer.layer = 3
	add_child(ui_layer)

	embody_btn = _make_button("◎  EMBODY", Color(0.20, 0.55, 0.85))
	embody_btn.pressed.connect(_embody)
	ui_layer.add_child(embody_btn)

	surface_btn = _make_button("▲  SURFACE", Color(0.30, 0.45, 0.40))
	surface_btn.pressed.connect(_surface)
	ui_layer.add_child(surface_btn)

	fire_btn = _make_button("● FIRE", Color(0.80, 0.25, 0.20))
	fire_btn.pressed.connect(_fire)
	ui_layer.add_child(fire_btn)

	hint_label = Label.new()
	hint_label.add_theme_font_size_override("font_size", 18)
	hint_label.add_theme_color_override("font_color", Color(0.8, 0.85, 0.8))
	ui_layer.add_child(hint_label)


func _make_button(txt: String, tint: Color) -> Button:
	var b := Button.new()
	b.text = txt
	b.add_theme_font_size_override("font_size", 26)
	var sb := StyleBoxFlat.new()
	sb.bg_color = tint
	sb.set_corner_radius_all(14)
	sb.set_content_margin_all(16)
	b.add_theme_stylebox_override("normal", sb)
	var sbh := sb.duplicate()
	sbh.bg_color = tint.lightened(0.15)
	b.add_theme_stylebox_override("hover", sbh)
	b.add_theme_stylebox_override("pressed", sbh)
	b.add_theme_color_override("font_color", Color.WHITE)
	return b


# ───────────────────────────────── layout / resize ───────────────────────
func _on_resize() -> void:
	var vp := get_viewport().get_visible_rect().size
	base_scale = min(vp.x, vp.y) / FIELD_SIZE
	# rebuild map geometry that depends on base_scale by rescaling map_root instead:
	# we keep map_root child coords fixed at the INITIAL base_scale, and fit via zoom.
	# Simpler: recompute the map fit transform now.
	_fit_map(vp)
	_layout_ui(vp)


func _fit_map(vp: Vector2) -> void:
	# map_root children were authored at base_scale px/m (captured at first build).
	# Center the field and apply the current zoom via map_root.scale.
	if map_root == null:
		return
	var fit: float = min(vp.x, vp.y) / (FIELD_SIZE * base_scale) * 0.9
	map_root.scale = Vector2(fit, fit)
	map_root.position = vp * 0.5 - Vector2(FIELD_SIZE, FIELD_SIZE) * base_scale * 0.5 * fit


func _layout_ui(vp: Vector2) -> void:
	embody_btn.position = Vector2(vp.x * 0.5 - 90, vp.y - 80)
	embody_btn.size = Vector2(180, 60)
	surface_btn.position = Vector2(20, 20)
	surface_btn.size = Vector2(180, 56)
	fire_btn.position = Vector2(vp.x - 180, vp.y - 150)
	fire_btn.size = Vector2(150, 110)
	blind_label.position = Vector2(vp.x * 0.5 - 150, 22)
	alert_label.position = Vector2(0, vp.y * 0.30)
	alert_label.size = Vector2(vp.x, 40)
	crosshair.position = vp * 0.5
	crosshair.polygon = PackedVector2Array([
		Vector2(-10, -1.5), Vector2(-1.5, -1.5), Vector2(-1.5, -10), Vector2(1.5, -10),
		Vector2(1.5, -1.5), Vector2(10, -1.5), Vector2(10, 1.5), Vector2(1.5, 1.5),
		Vector2(1.5, 10), Vector2(-1.5, 10), Vector2(-1.5, 1.5), Vector2(-10, 1.5),
	])
	hint_label.position = Vector2(20, vp.y - 36)
	_refresh_ui_rects()


func _refresh_ui_rects() -> void:
	ui_rects = [
		embody_btn.get_global_rect(),
		surface_btn.get_global_rect(),
		fire_btn.get_global_rect(),
	]


# ───────────────────────────────── mode swap ─────────────────────────────
func _embody() -> void:
	if mode == Mode.EMBODIED:
		return
	mode = Mode.EMBODIED
	move_target = null            # you took the wheel — the standing order stops
	alert_timer = ALERT_PERIOD * 0.6
	Input.vibrate_handheld(35)    # a tactile "dive" thunk
	_apply_mode()


func _surface() -> void:
	if mode == Mode.COMMAND:
		return
	mode = Mode.COMMAND
	joy_index = -1
	look_index = -1
	joy_vec = Vector2.ZERO
	alert_show = 0.0
	_apply_mode()


func _apply_mode() -> void:
	var embodied := mode == Mode.EMBODIED
	map_layer.visible = not embodied
	fx_layer.visible = embodied
	embody_btn.visible = not embodied
	surface_btn.visible = embodied
	fire_btn.visible = embodied
	joy_base.visible = false
	joy_knob.visible = false
	alert_label.visible = false
	hint_label.text = "COMMAND: tap to move · drag to pan · pinch to zoom · EMBODY to dive" if not embodied \
		else "EMBODIED: left thumb = move · right drag = look · FIRE · you are blind"
	_refresh_ui_rects()


# ───────────────────────────────── input ─────────────────────────────────
func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventScreenTouch:
		if event.pressed:
			_on_press(event.index, event.position)
		else:
			_on_release(event.index, event.position)
	elif event is InputEventScreenDrag:
		_on_drag(event.index, event.position, event.relative)


func _over_ui(p: Vector2) -> bool:
	for r in ui_rects:
		if r.has_point(p):
			return true
	return false


func _on_press(index: int, pos: Vector2) -> void:
	touches[index] = { "pos": pos, "start": pos, "moved": 0.0 }
	if mode == Mode.EMBODIED:
		if _over_ui(pos):
			return
		var half := get_viewport().get_visible_rect().size.x * 0.5
		if pos.x < half and joy_index == -1:
			joy_index = index
			joy_base.position = pos
			joy_knob.position = pos
			joy_base.visible = true
			joy_knob.visible = true
		elif look_index == -1:
			look_index = index


func _on_drag(index: int, pos: Vector2, rel: Vector2) -> void:
	if touches.has(index):
		touches[index].moved += rel.length()
		touches[index].pos = pos

	if mode == Mode.EMBODIED:
		if index == joy_index:
			var off := pos - joy_base.position
			if off.length() > JOY_RADIUS:
				off = off.normalized() * JOY_RADIUS
			joy_vec = off
			joy_knob.position = joy_base.position + off
		elif index == look_index:
			unit_yaw -= deg_to_rad(rel.x * LOOK_SENS_DEG)
			look_pitch = clamp(look_pitch - rel.y * LOOK_SENS_DEG, -85.0, 85.0)
		return

	# COMMAND: 1 finger = pan, 2 fingers = pinch-zoom about the centroid
	if touches.size() >= 2:
		_pinch()
	elif touches.size() == 1:
		map_root.position += rel


func _pinch() -> void:
	var keys := touches.keys()
	var a: Vector2 = touches[keys[0]].pos
	var b: Vector2 = touches[keys[1]].pos
	var centroid := (a + b) * 0.5
	var dist := a.distance_to(b)
	if not has_meta("pinch_dist"):
		set_meta("pinch_dist", dist)
		set_meta("pinch_centroid", centroid)
		return
	var prev_dist: float = get_meta("pinch_dist")
	var prev_centroid: Vector2 = get_meta("pinch_centroid")
	if prev_dist > 1.0:
		var factor: float = clamp(dist / prev_dist, 0.5, 2.0)
		var new_scale := map_root.scale * factor
		var z: float = clamp(new_scale.x, _fit_floor(), _fit_floor() * (ZOOM_MAX / ZOOM_MIN))
		factor = z / map_root.scale.x
		map_root.scale = Vector2(z, z)
		map_root.position = centroid - (centroid - map_root.position) * factor
	map_root.position += centroid - prev_centroid
	set_meta("pinch_dist", dist)
	set_meta("pinch_centroid", centroid)


func _fit_floor() -> float:
	var vp := get_viewport().get_visible_rect().size
	return min(vp.x, vp.y) / (FIELD_SIZE * base_scale) * 0.9 * ZOOM_MIN


func _on_release(index: int, pos: Vector2) -> void:
	var was_tap := false
	if touches.has(index):
		was_tap = touches[index].moved < 12.0
		touches.erase(index)
	if touches.size() < 2:
		if has_meta("pinch_dist"):
			remove_meta("pinch_dist")
		if has_meta("pinch_centroid"):
			remove_meta("pinch_centroid")

	if mode == Mode.EMBODIED:
		if index == joy_index:
			joy_index = -1
			joy_vec = Vector2.ZERO
			joy_base.visible = false
			joy_knob.visible = false
		elif index == look_index:
			look_index = -1
		return

	# COMMAND: a clean tap issues a move order (literal executor walks straight there)
	if was_tap and not _over_ui(pos):
		var field := map_root.to_local(pos) / base_scale
		field.x = clamp(field.x, 0.5, FIELD_SIZE - 0.5)
		field.y = clamp(field.y, 0.5, FIELD_SIZE - 0.5)
		move_target = field
		target_marker.position = field * base_scale
		target_marker.visible = true


# ───────────────────────────────── per-frame ─────────────────────────────
func _process(delta: float) -> void:
	if mode == Mode.COMMAND:
		_tick_command(delta)
	else:
		_tick_embodied(delta)
	_sync_views()


func _tick_command(delta: float) -> void:
	# literal executor: step straight toward the standing order, stop when reached
	if move_target != null:
		var to: Vector2 = move_target - unit_pos
		var d := to.length()
		if d < 0.15:
			move_target = null
			target_marker.visible = false
		else:
			face_vec = to / d
			unit_pos += face_vec * min(UNIT_SPEED * delta, d)
			unit_yaw = atan2(face_vec.x, -face_vec.y)


func _tick_embodied(delta: float) -> void:
	# move relative to where we're looking
	var fwd_in := -joy_vec.y / JOY_RADIUS
	var strafe_in := joy_vec.x / JOY_RADIUS
	var fwd := face_vec
	var right := Vector2(-face_vec.y, face_vec.x)
	var dir := fwd * fwd_in + right * strafe_in
	if dir.length() > 0.01:
		var want: Vector2 = unit_pos + dir.normalized() * FPS_SPEED * delta * clamp(dir.length(), 0.0, 1.0)
		unit_pos = _slide(unit_pos, want)

	# faked "something's wrong back home" — a DIRECTION + haptic, never the map (§6)
	alert_timer -= delta
	if alert_timer <= 0.0:
		alert_timer = ALERT_PERIOD
		alert_dir_index = (alert_dir_index + 1) % ALERT_DIRS.size()
		alert_show = 2.2
		Input.vibrate_handheld(220)
	if alert_show > 0.0:
		alert_show -= delta
		alert_label.text = "⚠  Commander — taking fire on %s" % ALERT_DIRS[alert_dir_index]
		alert_label.visible = true
		alert_flash.color.a = clamp(alert_show, 0.0, 0.5) * 0.6
	else:
		alert_label.visible = false
		alert_flash.color.a = 0.0


func _slide(from: Vector2, to: Vector2) -> Vector2:
	to.x = clamp(to.x, 0.4, FIELD_SIZE - 0.4)
	to.y = clamp(to.y, 0.4, FIELD_SIZE - 0.4)
	for r in cover:
		var grown: Rect2 = r.grow(0.4)
		if grown.has_point(to):
			# block whichever axis caused entry (cheap separation)
			var only_x := Vector2(to.x, from.y)
			var only_y := Vector2(from.x, to.y)
			if not grown.has_point(only_x):
				return only_x
			if not grown.has_point(only_y):
				return only_y
			return from
	return to


func _sync_views() -> void:
	# 3D camera rides the shared entity (first person: own mesh stays hidden)
	cam3d.position = Vector3(unit_pos.x, EYE_HEIGHT, unit_pos.y)
	cam3d.rotation = Vector3(deg_to_rad(look_pitch), unit_yaw, 0.0)
	if mode == Mode.EMBODIED:
		var lf := -cam3d.global_transform.basis.z
		var flat := Vector2(lf.x, lf.z)
		if flat.length() > 0.001:
			face_vec = flat.normalized()
	# 2D arrow rides the same entity
	unit_arrow.position = unit_pos * base_scale
	unit_arrow.rotation = atan2(face_vec.y, face_vec.x)


# ───────────────────────────────── firing ────────────────────────────────
func _fire() -> void:
	Input.vibrate_handheld(18)
	var best = null
	var best_d := INF
	for e_data in enemies:
		if not e_data.alive:
			continue
		var to: Vector2 = e_data.pos - unit_pos
		var d := to.length()
		if d > FIRE_RANGE or d < 0.01:
			continue
		var ang := rad_to_deg(abs(face_vec.angle_to(to.normalized())))
		if ang <= FIRE_CONE_DEG and d < best_d:
			best_d = d
			best = e_data
	if best != null:
		best.alive = false
		best.mesh.visible = false
		best.dot.visible = false
		_muzzle_confirm(true)
	else:
		_muzzle_confirm(false)


func _muzzle_confirm(hit: bool) -> void:
	crosshair.color = Color(1.0, 0.3, 0.2, 0.95) if hit else Color(1, 1, 1, 0.7)
	var t := create_tween()
	t.tween_interval(0.12)
	t.tween_callback(func(): crosshair.color = Color(1, 1, 1, 0.7))


# ───────────────────────────────── helpers ───────────────────────────────
func _circle(radius: float, color: Color, segments: int = 28) -> Polygon2D:
	var poly := Polygon2D.new()
	var pts := PackedVector2Array()
	for i in range(segments):
		var a := TAU * float(i) / float(segments)
		pts.append(Vector2(cos(a), sin(a)) * radius)
	poly.polygon = pts
	poly.color = color
	return poly
