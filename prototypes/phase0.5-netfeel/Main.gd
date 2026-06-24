extends Node
# ═══════════════════════════════════════════════════════════════════════════════
#  GOING DARK — Phase 0.5 netfeel spike  (THROWAWAY)
# ───────────────────────────────────────────────────────────────────────────────
#  Feel-test embodied 1v1 FPS combat over deterministic-lockstep + input-delay
#  netcode, to resolve open-questions Q7 (netcode model) and Q8 (tick rate) BEFORE
#  the Rust engine spine. Disposable Godot — see docs/phase-0.5-plan.md.
#
#  NON-GOALS (plan §2): not the engine; NOT a determinism test (floats are fine
#  here — both peers run identical code on the same exchanged inputs, we do not
#  checksum); no 200-unit sim; no real netcode stack; no audio. 1v1 feel only.
#
#  The model under test:
#   * Lockstep: peers exchange per-tick INPUT commands (not state) and both simulate
#     both avatars at a fixed tick. An input sampled at tick T executes at T+D.
#   * MODE A (pure lockstep): your own camera/aim/move lag by D ticks. The baseline.
#   * MODE B (avatar-local prediction): your camera/aim respond immediately and your
#     movement is predicted; the authoritative result still resolves at T+D and is
#     what fire/the remote view use. Prediction never feeds shared sim state.
#   * A latency INJECTOR adds tunable RTT/jitter/loss on the send path, because a LAN
#     is otherwise ~0 ms and would flatter lockstep.
# ═══════════════════════════════════════════════════════════════════════════════

const PORT := 8757
const FIELD := 40.0
const EYE := 1.6
const SPEED := 4.5
const RADIUS := 0.5
const LOOK_SENS_DEG := 0.22
const FIRE_RANGE := 35.0
const FIRE_CONE := 0.6          # metres of aim assist at range (perp tolerance)
const JOY_R := 130.0
const W := 6                    # redundant input window per packet
const MAX_HP := 5
const RESPAWN_TICKS := 60

# ── Latency presets the NET button cycles (plan §7) ──
const PRESETS := [
	{ "n": "LAN 0ms",        "rtt": 0.0,   "jit": 0.0,  "loss": 0.0 },
	{ "n": "40ms clean",     "rtt": 40.0,  "jit": 0.0,  "loss": 0.0 },
	{ "n": "80ms clean",     "rtt": 80.0,  "jit": 0.0,  "loss": 0.0 },
	{ "n": "120ms clean",    "rtt": 120.0, "jit": 0.0,  "loss": 0.0 },
	{ "n": "80ms jit/loss",  "rtt": 80.0,  "jit": 15.0, "loss": 0.02 },
	{ "n": "160ms jit/loss", "rtt": 160.0, "jit": 20.0, "loss": 0.03 },
]

enum Role { NONE, HOST, CLIENT }
var role: int = Role.NONE
var my_av := 0                  # host drives avatar 0, client avatar 1
var started := false

# ── netcode clocks ──
var tick_hz := 30
var send_tick := 0
var sim_tick := 0
var acc := 0.0
var local_inputs: Dictionary = {}    # sample_tick -> cmd
var remote_inputs: Dictionary = {}   # sample_tick -> cmd

# ── injector params (from preset) ──
var rtt := 0.0
var jit := 0.0
var loss := 0.0
var preset_i := 0

# ── feel mode ──
var mode_b := true                   # false = pure lockstep, true = avatar-local prediction

# ── avatars (authoritative) ──
var avatars: Array = []              # 2 dicts
var spawn := [Vector2(20, 6), Vector2(20, 34)]
var spawn_yaw := [PI, 0.0]

# ── local prediction / live aim (Mode B) ──
var cam_yaw := PI
var cam_pitch := 0.0
var render_pos := Vector2(20, 6)
var look_accum := Vector2.ZERO       # raw look delta buffered for the next sample

# ── outbound latency link ──
var out_queue: Array = []            # {release:int, data:Array}

# ── 3D / UI nodes ──
var world: Node3D
var cam3d: Camera3D
var cover: Array = []
var hud: CanvasLayer
var diag: Label
var hitmark: Polygon2D
var crosshair: Polygon2D
var joy_base: Polygon2D
var joy_knob: Polygon2D
var lobby: Control
var ip_edit: LineEdit
var mode_btn: Button
var tick_btn: Button
var net_btn: Button
var fire_btn: Button

# ── touch ──
var touches: Dictionary = {}
var joy_index := -1
var look_index := -1
var joy_vec := Vector2.ZERO
var fire_latched := false
var ui_rects: Array = []


func _ready() -> void:
	randomize()
	_apply_preset(0)
	_build_world()
	_build_arena_and_avatars()
	_build_hud()
	_build_lobby()
	get_viewport().size_changed.connect(_relayout)
	_relayout()
	_auto_start_from_cmdline()


# ───────────────────────────── world / arena ─────────────────────────────
func _build_world() -> void:
	world = Node3D.new()
	add_child(world)
	var env := WorldEnvironment.new()
	var e := Environment.new()
	e.background_mode = Environment.BG_COLOR
	e.background_color = Color(0.46, 0.58, 0.72)
	e.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	e.ambient_light_color = Color(0.55, 0.58, 0.62)
	e.ambient_light_energy = 1.0
	env.environment = e
	world.add_child(env)
	var sun := DirectionalLight3D.new()
	sun.rotation = Vector3(deg_to_rad(-55), deg_to_rad(40), 0)
	world.add_child(sun)
	var ground := MeshInstance3D.new()
	var pm := PlaneMesh.new()
	pm.size = Vector2(FIELD, FIELD)
	ground.mesh = pm
	ground.position = Vector3(FIELD * 0.5, 0, FIELD * 0.5)
	ground.material_override = _mat(Color(0.30, 0.40, 0.26))
	world.add_child(ground)
	cam3d = Camera3D.new()
	cam3d.fov = 75.0
	cam3d.near = 0.05
	world.add_child(cam3d)


func _build_arena_and_avatars() -> void:
	cover = [Rect2(12, 16, 4, 3), Rect2(24, 16, 4, 3), Rect2(18, 9, 3, 3), Rect2(18, 28, 3, 3)]
	for r in cover:
		var b := MeshInstance3D.new()
		var bm := BoxMesh.new()
		bm.size = Vector3(r.size.x, 2.4, r.size.y)
		b.mesh = bm
		b.position = Vector3(r.position.x + r.size.x * 0.5, 1.2, r.position.y + r.size.y * 0.5)
		b.material_override = _mat(Color(0.50, 0.48, 0.45))
		world.add_child(b)
	var cols := [Color(0.35, 0.6, 1.0), Color(0.95, 0.5, 0.2)]
	avatars.clear()
	for i in range(2):
		var m := MeshInstance3D.new()
		var cm := CapsuleMesh.new()
		cm.radius = RADIUS
		cm.height = 1.8
		m.mesh = cm
		m.material_override = _mat(cols[i])
		world.add_child(m)
		avatars.append({
			"pos": spawn[i], "yaw": spawn_yaw[i], "pitch": 0.0,
			"ppos": spawn[i], "pyaw": spawn_yaw[i], "ppitch": 0.0,
			"hp": MAX_HP, "down": 0, "mesh": m, "hit_t": 0,
		})


func _mat(c: Color) -> StandardMaterial3D:
	var m := StandardMaterial3D.new()
	m.albedo_color = c
	m.roughness = 0.9
	return m


# ───────────────────────────── HUD / lobby ───────────────────────────────
func _build_hud() -> void:
	hud = CanvasLayer.new()
	hud.layer = 2
	add_child(hud)

	crosshair = _plus(Color(1, 1, 1, 0.8))
	hud.add_child(crosshair)
	hitmark = _plus(Color(1, 0.3, 0.2, 0.0), 16.0)
	hud.add_child(hitmark)

	joy_base = _circle(JOY_R, Color(1, 1, 1, 0.10))
	joy_base.visible = false
	hud.add_child(joy_base)
	joy_knob = _circle(JOY_R * 0.42, Color(1, 1, 1, 0.30))
	joy_knob.visible = false
	hud.add_child(joy_knob)

	diag = Label.new()
	diag.add_theme_font_size_override("font_size", 18)
	diag.add_theme_color_override("font_color", Color(0.85, 0.95, 0.85))
	diag.add_theme_color_override("font_outline_color", Color(0, 0, 0))
	diag.add_theme_constant_override("outline_size", 4)
	diag.visible = false
	hud.add_child(diag)

	mode_btn = _btn("MODE", Color(0.25, 0.45, 0.7), _toggle_mode)
	tick_btn = _btn("30Hz", Color(0.3, 0.45, 0.4), _toggle_tick)
	net_btn = _btn("NET", Color(0.45, 0.4, 0.25), _cycle_net)
	fire_btn = _btn("● FIRE", Color(0.8, 0.25, 0.2), _fire)
	for b in [mode_btn, tick_btn, net_btn, fire_btn]:
		b.visible = false
		hud.add_child(b)


func _build_lobby() -> void:
	lobby = Control.new()
	lobby.anchor_right = 1.0
	lobby.anchor_bottom = 1.0
	lobby.mouse_filter = Control.MOUSE_FILTER_IGNORE
	hud.add_child(lobby)

	var title := Label.new()
	title.text = "Going Dark — Phase 0.5 netfeel (throwaway)\nHOST on one device, JOIN from the other (same Wi-Fi)."
	title.add_theme_font_size_override("font_size", 20)
	title.position = Vector2(40, 30)
	lobby.add_child(title)

	var ips := Label.new()
	ips.text = "this device: " + ", ".join(PackedStringArray(_local_ips()))
	ips.add_theme_font_size_override("font_size", 16)
	ips.position = Vector2(40, 90)
	lobby.add_child(ips)

	ip_edit = LineEdit.new()
	ip_edit.text = "127.0.0.1"
	ip_edit.placeholder_text = "host IP"
	ip_edit.position = Vector2(40, 130)
	ip_edit.size = Vector2(280, 48)
	lobby.add_child(ip_edit)

	var hbtn := _btn("HOST", Color(0.25, 0.5, 0.35), host)
	hbtn.position = Vector2(40, 190)
	hbtn.size = Vector2(150, 60)
	lobby.add_child(hbtn)

	var jbtn := _btn("JOIN", Color(0.3, 0.4, 0.6), _join_pressed)
	jbtn.position = Vector2(200, 190)
	jbtn.size = Vector2(150, 60)
	lobby.add_child(jbtn)


func _local_ips() -> Array:
	var out: Array = []
	for a in IP.get_local_addresses():
		var s := str(a)
		if s.begins_with("192.168.") or s.begins_with("10.") or s.begins_with("172."):
			out.append(s)
	if out.is_empty():
		out.append("127.0.0.1")
	return out


# ───────────────────────────── networking ────────────────────────────────
func host() -> void:
	var peer := ENetMultiplayerPeer.new()
	var err := peer.create_server(PORT, 1)
	if err != OK:
		_flash_title("server error %d" % err)
		return
	multiplayer.multiplayer_peer = peer
	multiplayer.peer_connected.connect(_on_peer)
	multiplayer.peer_disconnected.connect(_on_drop)
	role = Role.HOST
	my_av = 0
	_flash_title("hosting on :%d — waiting for join…" % PORT)


func _join_pressed() -> void:
	join(ip_edit.text.strip_edges())


func join(ip: String) -> void:
	var peer := ENetMultiplayerPeer.new()
	var err := peer.create_client(ip, PORT)
	if err != OK:
		_flash_title("client error %d" % err)
		return
	multiplayer.multiplayer_peer = peer
	multiplayer.peer_connected.connect(_on_peer)
	multiplayer.peer_disconnected.connect(_on_drop)
	multiplayer.connection_failed.connect(func(): _flash_title("connection failed"))
	role = Role.CLIENT
	my_av = 1
	_flash_title("joining %s…" % ip)


func _on_peer(_id: int) -> void:
	_start_match()


func _on_drop(_id: int) -> void:
	started = false
	_flash_title("peer dropped — back to lobby")
	lobby.visible = true
	for b in [mode_btn, tick_btn, net_btn, fire_btn]:
		b.visible = false
	diag.visible = false


func _start_match() -> void:
	send_tick = 0
	sim_tick = 0
	acc = 0.0
	local_inputs.clear()
	remote_inputs.clear()
	out_queue.clear()
	for i in range(2):
		var a: Dictionary = avatars[i]
		a.pos = spawn[i]; a.ppos = spawn[i]
		a.yaw = spawn_yaw[i]; a.pyaw = spawn_yaw[i]
		a.pitch = 0.0; a.ppitch = 0.0
		a.hp = MAX_HP; a.down = 0
	cam_yaw = spawn_yaw[my_av]
	cam_pitch = 0.0
	render_pos = spawn[my_av]
	started = true
	print("match start, role=", role, " my_av=", my_av)
	lobby.visible = false
	diag.visible = true
	for b in [mode_btn, tick_btn, net_btn, fire_btn]:
		b.visible = true
	_relayout()


@rpc("any_peer", "unreliable", "call_remote")
func _recv(list: Array) -> void:
	for e in list:
		var t: int = int(e[0])
		if not remote_inputs.has(t):
			remote_inputs[t] = { "move": Vector2(e[1], e[2]), "look": Vector2(e[3], e[4]), "fire": e[5] > 0 }


func _queue_send(s: int) -> void:
	var list: Array = []
	var start: int = max(0, s - W + 1)
	for i in range(start, s + 1):
		if not local_inputs.has(i):
			continue
		var c: Dictionary = local_inputs[i]
		var mv: Vector2 = c.move
		var lk: Vector2 = c.look
		list.append([i, mv.x, mv.y, lk.x, lk.y, (1 if c.fire else 0)])
	# loss + delay + jitter, modelled on the send side
	if randf() < loss:
		return
	var d: float = rtt * 0.5 + randf_range(-jit, jit)
	if d < 0.0:
		d = 0.0
	out_queue.append({ "release": Time.get_ticks_msec() + int(d), "data": list })


func _pump_out() -> void:
	if multiplayer.multiplayer_peer == null:
		return
	var now := Time.get_ticks_msec()
	var keep: Array = []
	for p in out_queue:
		if int(p.release) <= now:
			if multiplayer.has_multiplayer_peer() and multiplayer.get_peers().size() > 0:
				_recv.rpc(p.data)
		else:
			keep.append(p)
	out_queue = keep


# ───────────────────────────── tick driver ───────────────────────────────
func _input_delay() -> int:
	var period_ms: float = 1000.0 / float(tick_hz)
	var d: int = int(ceil((rtt * 0.5 + jit + 5.0) / period_ms))
	return clampi(d, 1, 8)


func _process(dt: float) -> void:
	_pump_out()
	if not started:
		return
	# live aim integration (Mode B gets immediate look; Mode A camera follows sim)
	if mode_b:
		cam_yaw -= deg_to_rad(look_accum_frame.x * LOOK_SENS_DEG)
		cam_pitch = clampf(cam_pitch - look_accum_frame.y * LOOK_SENS_DEG, -85.0, 85.0)
	look_accum += look_accum_frame
	look_accum_frame = Vector2.ZERO

	var period: float = 1.0 / float(tick_hz)
	acc += dt
	var guard := 0
	while acc >= period and guard < 6:
		acc -= period
		_tick()
		guard += 1
	_render(clampf(acc / period, 0.0, 1.0))
	_update_diag()


var look_accum_frame := Vector2.ZERO

func _tick() -> void:
	var d := _input_delay()
	# 1) sample + send local input for send_tick (always — even while stalled)
	local_inputs[send_tick] = { "move": _stick_move(), "look": look_accum, "fire": fire_latched }
	look_accum = Vector2.ZERO
	fire_latched = false
	_queue_send(send_tick)
	send_tick += 1
	# 2) simulate as far as the exchanged inputs allow (stall = lockstep pacing)
	while sim_tick < send_tick and _can_sim(sim_tick, d):
		_sim_tick(sim_tick, d)
		sim_tick += 1
	_prune(d)


func _can_sim(t: int, d: int) -> bool:
	var need := t - d
	if need < 0:
		return true
	return remote_inputs.has(need)


func _sim_tick(t: int, d: int) -> void:
	var li := _cmd_at(local_inputs, t - d)
	var ri := _cmd_at(remote_inputs, t - d)
	var a_local: Dictionary = avatars[my_av]
	var a_remote: Dictionary = avatars[1 - my_av]
	_store_prev(a_local)
	_store_prev(a_remote)
	if a_local.down > 0: a_local.down -= 1
	if a_remote.down > 0: a_remote.down -= 1
	_apply(a_local, li)
	_apply(a_remote, ri)
	_fire_resolve(a_local, a_remote, li)
	_fire_resolve(a_remote, a_local, ri)


func _cmd_at(buf: Dictionary, t: int) -> Dictionary:
	if t < 0 or not buf.has(t):
		return { "move": Vector2.ZERO, "look": Vector2.ZERO, "fire": false }
	return buf[t]


func _store_prev(a: Dictionary) -> void:
	a.ppos = a.pos
	a.pyaw = a.yaw
	a.ppitch = a.pitch


func _apply(a: Dictionary, c: Dictionary) -> void:
	if a.down > 0:
		return
	var lk: Vector2 = c.look
	a.yaw = float(a.yaw) - deg_to_rad(lk.x * LOOK_SENS_DEG)
	a.pitch = clampf(float(a.pitch) - lk.y * LOOK_SENS_DEG, -85.0, 85.0)
	var mv: Vector2 = c.move
	if mv.length() > 0.01:
		var dir: Vector2 = _fwd(a.yaw) * mv.y + _right(a.yaw) * mv.x
		var step: float = SPEED * (1.0 / float(tick_hz)) * clampf(mv.length(), 0.0, 1.0)
		a.pos = _collide(a.pos, a.pos + dir.normalized() * step)


func _fire_resolve(shooter: Dictionary, target: Dictionary, c: Dictionary) -> void:
	if not c.fire or shooter.down > 0 or target.down > 0:
		return
	var sp: Vector2 = shooter.pos
	var tp: Vector2 = target.pos
	var aim: Vector2 = _fwd(shooter.yaw)
	var to: Vector2 = tp - sp
	var proj: float = to.dot(aim)
	if proj <= 0.0 or proj > FIRE_RANGE:
		return
	var perp: float = (to - aim * proj).length()
	if perp > RADIUS + FIRE_CONE:
		return
	if _blocked(sp, tp):
		return
	target.hp = int(target.hp) - 1
	target.hit_t = Time.get_ticks_msec()
	if int(target.hp) <= 0:
		target.hp = MAX_HP
		target.down = RESPAWN_TICKS
		target.pos = spawn[avatars.find(target)]


func _prune(d: int) -> void:
	# drop inputs older than we could ever need again
	var floor_t: int = sim_tick - d - W - 4
	if floor_t < 0:
		return
	for buf in [local_inputs, remote_inputs]:
		for k in buf.keys():
			if int(k) < floor_t:
				buf.erase(k)


# ───────────────────────────── render ────────────────────────────────────
func _render(alpha: float) -> void:
	var a_remote: Dictionary = avatars[1 - my_av]
	# remote avatar: interpolate authoritative ticks
	var rp: Vector2 = _lerp2(a_remote.ppos, a_remote.pos, alpha)
	a_remote.mesh.position = Vector3(rp.x, 0.9, rp.y)
	a_remote.mesh.visible = a_remote.down == 0
	avatars[my_av].mesh.visible = false  # first person: hide own body

	var a_local: Dictionary = avatars[my_av]
	var eye_pos: Vector2
	var yaw: float
	var pitch: float
	if mode_b:
		# predicted position (optimistic replay of pending local inputs) + live aim
		eye_pos = _predict_local()
		render_pos = render_pos.lerp(eye_pos, 0.5)   # smoothing → rubber-band on mispredict
		eye_pos = render_pos
		yaw = cam_yaw
		pitch = cam_pitch
	else:
		# pure lockstep: camera rides the delayed authoritative avatar
		eye_pos = _lerp2(a_local.ppos, a_local.pos, alpha)
		yaw = lerp_angle(float(a_local.pyaw), float(a_local.yaw), alpha)
		pitch = lerpf(float(a_local.ppitch), float(a_local.pitch), alpha)
	cam3d.position = Vector3(eye_pos.x, EYE, eye_pos.y)
	cam3d.rotation = Vector3(deg_to_rad(pitch), yaw, 0.0)

	# hit marker fades after you land a shot on the enemy
	var since: int = Time.get_ticks_msec() - int(a_remote.hit_t)
	var hc := hitmark.color
	hc.a = clampf(1.0 - since / 250.0, 0.0, 0.9)
	hitmark.color = hc


func _predict_local() -> Vector2:
	var a: Dictionary = avatars[my_av]
	var p: Vector2 = a.pos
	if a.down > 0:
		return p
	var d := _input_delay()
	var from_t: int = max(0, sim_tick - d)
	for s in range(from_t, send_tick):
		if not local_inputs.has(s):
			continue
		var c: Dictionary = local_inputs[s]
		var mv: Vector2 = c.move
		if mv.length() > 0.01:
			var dir: Vector2 = _fwd(cam_yaw) * mv.y + _right(cam_yaw) * mv.x
			var step: float = SPEED * (1.0 / float(tick_hz)) * clampf(mv.length(), 0.0, 1.0)
			p = _collide(p, p + dir.normalized() * step)  # optimistic: ignores remote avatar
	return p


# ───────────────────────────── geometry helpers ──────────────────────────
func _fwd(yaw: float) -> Vector2:
	return Vector2(-sin(yaw), -cos(yaw))

func _right(yaw: float) -> Vector2:
	return Vector2(cos(yaw), -sin(yaw))

func _lerp2(a: Vector2, b: Vector2, t: float) -> Vector2:
	return a.lerp(b, t)

func _collide(from: Vector2, to: Vector2) -> Vector2:
	to.x = clampf(to.x, RADIUS, FIELD - RADIUS)
	to.y = clampf(to.y, RADIUS, FIELD - RADIUS)
	for r in cover:
		var g: Rect2 = (r as Rect2).grow(RADIUS)
		if g.has_point(to):
			var only_x := Vector2(to.x, from.y)
			var only_y := Vector2(from.x, to.y)
			if not g.has_point(only_x): return only_x
			if not g.has_point(only_y): return only_y
			return from
	return to

func _blocked(a: Vector2, b: Vector2) -> bool:
	for r in cover:
		if _seg_rect(a, b, r as Rect2):
			return true
	return false

func _seg_rect(p1: Vector2, p2: Vector2, r: Rect2) -> bool:
	# cheap: sample a few points along the segment
	for i in range(1, 8):
		var q: Vector2 = p1.lerp(p2, float(i) / 8.0)
		if r.has_point(q):
			return true
	return false


# ───────────────────────────── input (touch) ─────────────────────────────
func _stick_move() -> Vector2:
	return Vector2(joy_vec.x / JOY_R, -joy_vec.y / JOY_R)

func _unhandled_input(event: InputEvent) -> void:
	if not started:
		return
	if event is InputEventScreenTouch:
		if event.pressed: _press(event.index, event.position)
		else: _release(event.index, event.position)
	elif event is InputEventScreenDrag:
		_drag(event.index, event.position, event.relative)

func _over_ui(p: Vector2) -> bool:
	for r in ui_rects:
		if (r as Rect2).has_point(p):
			return true
	return false

func _press(index: int, pos: Vector2) -> void:
	touches[index] = pos
	if _over_ui(pos):
		return
	var half: float = get_viewport().get_visible_rect().size.x * 0.5
	if pos.x < half and joy_index == -1:
		joy_index = index
		joy_base.position = pos
		joy_knob.position = pos
		joy_base.visible = true
		joy_knob.visible = true
	elif look_index == -1:
		look_index = index

func _drag(index: int, pos: Vector2, rel: Vector2) -> void:
	if index == joy_index:
		var off: Vector2 = pos - joy_base.position
		if off.length() > JOY_R:
			off = off.normalized() * JOY_R
		joy_vec = off
		joy_knob.position = joy_base.position + off
	elif index == look_index:
		look_accum_frame += rel

func _release(index: int, _pos: Vector2) -> void:
	touches.erase(index)
	if index == joy_index:
		joy_index = -1
		joy_vec = Vector2.ZERO
		joy_base.visible = false
		joy_knob.visible = false
	elif index == look_index:
		look_index = -1

func _fire() -> void:
	fire_latched = true
	Input.vibrate_handheld(15)
	crosshair.color = Color(1.0, 0.4, 0.2, 0.95)
	var t := create_tween()
	t.tween_interval(0.08)
	t.tween_callback(func(): crosshair.color = Color(1, 1, 1, 0.8))


# ───────────────────────────── buttons / diag ────────────────────────────
func _toggle_mode() -> void:
	mode_b = not mode_b
	if mode_b:
		cam_yaw = float(avatars[my_av].yaw)
		cam_pitch = float(avatars[my_av].pitch)
		render_pos = avatars[my_av].pos

func _toggle_tick() -> void:
	tick_hz = 60 if tick_hz == 30 else 30
	tick_btn.text = "%dHz" % tick_hz

func _cycle_net() -> void:
	_apply_preset((preset_i + 1) % PRESETS.size())

func _apply_preset(i: int) -> void:
	preset_i = i
	var p: Dictionary = PRESETS[i]
	rtt = p.rtt
	jit = p.jit
	loss = p.loss

func _update_diag() -> void:
	var d := _input_delay()
	var a: Dictionary = avatars[my_av]
	var en: Dictionary = avatars[1 - my_av]
	var modename := "B predict" if mode_b else "A lockstep"
	diag.text = "%s | MODE %s | %dHz | D=%d (%dms) | %s | lag s%d→%d=%d | HP you %d  enemy %d" % [
		("HOST" if role == Role.HOST else "CLIENT"), modename, tick_hz, d,
		int(d * 1000.0 / tick_hz), PRESETS[preset_i].n,
		sim_tick, send_tick, send_tick - sim_tick, int(a.hp), int(en.hp),
	]


# ───────────────────────────── layout ────────────────────────────────────
func _relayout() -> void:
	var vp: Vector2 = get_viewport().get_visible_rect().size
	crosshair.position = vp * 0.5
	hitmark.position = vp * 0.5
	diag.position = Vector2(16, 10)
	fire_btn.position = Vector2(vp.x - 180, vp.y - 150); fire_btn.size = Vector2(150, 110)
	mode_btn.position = Vector2(vp.x - 180, 16); mode_btn.size = Vector2(164, 52)
	tick_btn.position = Vector2(vp.x - 360, 16); tick_btn.size = Vector2(120, 52)
	net_btn.position = Vector2(vp.x - 360, 78); net_btn.size = Vector2(344, 52)
	net_btn.text = "NET: " + str(PRESETS[preset_i].n)
	_refresh_ui_rects()

func _refresh_ui_rects() -> void:
	ui_rects = [fire_btn.get_global_rect(), mode_btn.get_global_rect(),
		tick_btn.get_global_rect(), net_btn.get_global_rect()]


# ───────────────────────────── misc ──────────────────────────────────────
func _auto_start_from_cmdline() -> void:
	for arg in OS.get_cmdline_user_args():
		if arg == "--host":
			host()
		elif arg.begins_with("--join="):
			join(arg.substr(7))

func _flash_title(s: String) -> void:
	if is_instance_valid(lobby) and lobby.get_child_count() > 0:
		var l := lobby.get_child(0) as Label
		if l: l.text = s
	print(s)

func _btn(txt: String, tint: Color, cb: Callable) -> Button:
	var b := Button.new()
	b.text = txt
	b.add_theme_font_size_override("font_size", 22)
	var sb := StyleBoxFlat.new()
	sb.bg_color = tint
	sb.set_corner_radius_all(12)
	sb.set_content_margin_all(10)
	b.add_theme_stylebox_override("normal", sb)
	b.add_theme_color_override("font_color", Color.WHITE)
	b.pressed.connect(cb)
	return b

func _circle(radius: float, color: Color, seg: int = 28) -> Polygon2D:
	var poly := Polygon2D.new()
	var pts := PackedVector2Array()
	for i in range(seg):
		var ang: float = TAU * float(i) / float(seg)
		pts.append(Vector2(cos(ang), sin(ang)) * radius)
	poly.polygon = pts
	poly.color = color
	return poly

func _plus(color: Color, s: float = 10.0) -> Polygon2D:
	var poly := Polygon2D.new()
	poly.polygon = PackedVector2Array([
		Vector2(-s, -1.5), Vector2(-1.5, -1.5), Vector2(-1.5, -s), Vector2(1.5, -s),
		Vector2(1.5, -1.5), Vector2(s, -1.5), Vector2(s, 1.5), Vector2(1.5, 1.5),
		Vector2(1.5, s), Vector2(-1.5, s), Vector2(-1.5, 1.5), Vector2(-s, 1.5),
	])
	poly.color = color
	return poly
