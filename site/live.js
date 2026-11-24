// The site's live layer: reveals on scroll, a scroll meter, the hero signal and the mites.
// Everything here is decoration: without JavaScript (or with reduced motion) the page reads the same.
(() => {
  "use strict";
  const root = document.documentElement;
  root.classList.add("js");
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const ACCENTS = ["#0ED4F6", "#E44A08", "#59FCE1", "#6BA208", "#F0284A", "#C2ECB9", "#16B3EB", "#BADBFE",
    "#FACCFC", "#D9C6FE", "#FBB5B1", "#FFD1E3", "#C8FCF4", "#FFD4BF"];

  // ---- Reveals: sections and cards rise in as they enter, cards staggered along their row.
  const targets = [...document.querySelectorAll("section h2, .sub, .facts div, .card, .install article, .open p, .hero .eyebrow, .hero h1, .hero .lede, .hero .cta, .hero .fine")];
  targets.forEach((el, i) => {
    el.classList.add("reveal");
    if (el.classList.contains("card")) el.style.setProperty("--d", `${(i % 3) * 0.09}s`);
    if (el.parentElement.classList.contains("facts")) el.style.setProperty("--d", `${[...el.parentElement.children].indexOf(el) * 0.08}s`);
  });
  [...document.querySelectorAll(".hero .reveal")].forEach((el, i) => el.style.setProperty("--d", `${i * 0.09}s`));
  const seen = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      e.target.classList.add("in");
      const n = e.target.querySelector("[data-count]");
      if (n && !still) countUp(n);
      seen.unobserve(e.target);
    }
  }, { rootMargin: "0px 0px -8% 0px", threshold: 0.12 });
  targets.forEach((el) => seen.observe(el));

  function countUp(el) {
    const to = +el.dataset.count, t0 = performance.now();
    const step = (t) => {
      const k = Math.min(1, (t - t0) / 900);
      el.textContent = Math.round(to * (1 - Math.pow(1 - k, 3)));
      if (k < 1) requestAnimationFrame(step);
    };
    el.textContent = "0";
    requestAnimationFrame(step);
  }

  // ---- Scroll: the meter, the grid, the hero shot lifting flat, scroll speed for everything that moves.
  let lastY = scrollY, velocity = 0;
  const shot = document.querySelector(".hero-shot");
  function onScroll() {
    const max = Math.max(1, root.scrollHeight - innerHeight);
    root.style.setProperty("--progress", (scrollY / max).toFixed(4));
    root.style.setProperty("--scroll", scrollY.toFixed(0));
    if (shot) {
      const r = shot.getBoundingClientRect();
      const lift = Math.min(1, Math.max(0, 1 - (r.top - innerHeight * 0.25) / (innerHeight * 0.6)));
      root.style.setProperty("--lift", lift.toFixed(3));
    }
    for (const img of cardImgs) {
      const r = img.getBoundingClientRect();
      if (r.bottom < 0 || r.top > innerHeight) continue;
      const c = (r.top + r.height / 2 - innerHeight / 2) / innerHeight;
      img.style.setProperty("--par", `${(c * -14).toFixed(1)}px`);
    }
  }
  let cardImgs = [];
  addEventListener("load", () => { cardImgs = [...document.querySelectorAll(".card .shot img")]; onScroll(); });
  addEventListener("scroll", onScroll, { passive: true });
  addEventListener("resize", onScroll);
  onScroll();

  // ---- Cards lean toward the pointer, with a glow in the plugin's colour under it.
  if (matchMedia("(pointer: fine)").matches && !still) {
    document.addEventListener("pointermove", (e) => {
      const card = e.target.closest?.(".card");
      if (!card) return;
      const r = card.getBoundingClientRect();
      const px = (e.clientX - r.left) / r.width, py = (e.clientY - r.top) / r.height;
      card.style.setProperty("--tx", `${((px - 0.5) * 6).toFixed(2)}deg`);
      card.style.setProperty("--ty", `${((0.5 - py) * 6).toFixed(2)}deg`);
      card.style.setProperty("--mx", `${(px * 100).toFixed(1)}%`);
      card.style.setProperty("--my", `${(py * 100).toFixed(1)}%`);
    }, { passive: true });
  }

  if (still) return;

  // ---- Canvases.
  const dpr = () => Math.min(2, devicePixelRatio || 1);
  function fit(c) {
    const r = c.getBoundingClientRect(), k = dpr();
    c.width = Math.round(r.width * k); c.height = Math.round(r.height * k);
    return k;
  }

  // The hero signal: three soft-clipped waves, like a signal through a saturator,
  // brighter and wilder while the page moves.
  const sig = document.getElementById("signal");
  const sctx = sig.getContext("2d");
  let sk = fit(sig);
  const WAVES = [["#F0284A", 1.0, 0.0, 1.7], ["#0ED4F6", 0.7, 2.1, 1.25], ["#59FCE1", 0.5, 4.0, 2.3]];
  function drawSignal(t, excite) {
    const w = sig.width, h = sig.height;
    sctx.clearRect(0, 0, w, h);
    const drive = 1.2 + excite * 5;
    for (const [col, amp, ph, f] of WAVES) {
      sctx.beginPath();
      for (let x = 0; x <= w; x += 4 * sk) {
        const u = x / w;
        const env = Math.sin(Math.PI * u) ** 1.5;
        const raw = Math.sin(u * Math.PI * 2 * f * 3 + t * (0.8 + f * 0.3) + ph) + 0.35 * Math.sin(u * 37 + t * 2.3 + ph);
        const y = h / 2 + Math.tanh(raw * drive) / Math.tanh(drive) * amp * env * h * 0.32;
        x ? sctx.lineTo(x, y) : sctx.moveTo(x, y);
      }
      sctx.strokeStyle = col;
      sctx.globalAlpha = 0.16 + excite * 0.3;
      sctx.lineWidth = 1.6 * sk;
      sctx.shadowColor = col; sctx.shadowBlur = 14 * sk;
      sctx.stroke();
    }
    sctx.globalAlpha = 1;
  }

  // The mites: the logo's mite, eight legs walking on alternating tetrapods, legs keeping pace with the ground they cover.
  const LEGS = [[11.2, 14, 6.5, 9.5], [10.4, 17, 4.5, 15], [10.2, 20.5, 4.5, 21.5], [10.8, 24, 6, 28],
    [20.8, 14, 25.5, 9.5], [21.6, 17, 27.5, 15], [21.8, 20.5, 27.5, 21.5], [21.2, 24, 26, 28]];
  // Eight legs, alternating tetrapods: left 1 and 3 with right 2 and 4, then the others.
  const TRIPOD = [0, 1, 0, 1, 1, 0, 1, 0];
  const BODY = new Path2D("M16 11 C20.5 11 22 15 22 19 C22 24 19.5 28 16 28 C12.5 28 10 24 10 19 C10 15 11.5 11 16 11 Z");
  const HEAD = new Path2D("M18.6 7.4 A2.6 2.6 0 1 1 13.4 7.4 A2.6 2.6 0 1 1 18.6 7.4 Z");
  const SIG = new Path2D("M11.8 21 C14 21 14.6 16.5 16 16.5 C17.4 16.5 18 21 20.2 21");

  const cv = document.getElementById("mites");
  const ctx = cv.getContext("2d");
  let k = fit(cv);
  let W = innerWidth, H = innerHeight;
  const rnd = (a, b) => a + Math.random() * (b - a);
  const mites = [];
  function spawn(m, edge) {
    m.size = m.size || rnd(26, 46);
    m.depth = (m.size - 22) / 24; // bigger = nearer = rides the scroll more
    if (edge) {
      const side = Math.floor(rnd(0, 4));
      m.x = side === 0 ? -30 : side === 1 ? W + 30 : rnd(0, W);
      m.y = side === 2 ? -30 : side === 3 ? H + 30 : rnd(0, H);
      m.h = Math.atan2(H / 2 - m.y, W / 2 - m.x) + rnd(-0.8, 0.8);
    } else {
      m.x = rnd(0, W); m.y = rnd(0, H); m.h = rnd(0, Math.PI * 2);
    }
    m.base = rnd(22, 46); m.speed = 0; m.turn = 0; m.gait = rnd(0, 6.28);
    m.rest = rnd(0, 2); m.walk = rnd(2, 6); m.fade = 1;
    m.col = ACCENTS[Math.floor(rnd(0, ACCENTS.length))];
    return m;
  }
  function populate() {
    const want = Math.round(Math.min(22, Math.max(8, (W * H) / 70000)));
    while (mites.length < want) mites.push(spawn({}, false));
    mites.length = want;
  }
  populate();

  let pointer = { x: -1e4, y: -1e4 }, burst = null;
  addEventListener("pointermove", (e) => { pointer = { x: e.clientX, y: e.clientY }; }, { passive: true });
  addEventListener("pointerdown", (e) => { burst = { x: e.clientX, y: e.clientY, t: 0 }; }, { passive: true });
  addEventListener("resize", () => { k = fit(cv); sk = fit(sig); W = innerWidth; H = innerHeight; populate(); });

  function drawMite(m) {
    const s = m.size / 32;
    ctx.setTransform(k * s, 0, 0, k * s, 0, 0);
    ctx.translate(m.x / s, m.y / s);
    ctx.rotate(m.h + Math.PI / 2);
    ctx.translate(-16, -18);
    ctx.lineCap = "round";
    ctx.globalAlpha = (0.75 + m.depth * 0.25) * m.fade;
    if (m.fade < 0.02) return;
    ctx.strokeStyle = "#a3abb8"; ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let i = 0; i < 8; i++) {
      const [x0, y0, x1, y1] = LEGS[i];
      const a = Math.sin(m.gait + TRIPOD[i] * Math.PI) * 0.45 * Math.min(1, m.speed / 20);
      const lift = 1 - 0.18 * Math.max(0, Math.cos(m.gait + TRIPOD[i] * Math.PI)) * Math.min(1, m.speed / 20);
      const dx = (x1 - x0) * lift, dy = (y1 - y0) * lift;
      const c = Math.cos(a), sn = Math.sin(a);
      ctx.moveTo(x0, y0);
      ctx.lineTo(x0 + dx * c - dy * sn, y0 + dx * sn + dy * c);
    }
    ctx.stroke();
    ctx.strokeStyle = "#e8ebf0"; ctx.lineWidth = 1.6;
    ctx.fillStyle = "#0b0d11";
    ctx.fill(BODY); ctx.stroke(BODY); ctx.fill(HEAD); ctx.stroke(HEAD);
    ctx.strokeStyle = m.col; ctx.lineWidth = 1.8;
    ctx.shadowColor = m.col; ctx.shadowBlur = 12 * k;
    ctx.stroke(SIG);
    ctx.shadowBlur = 0;
  }

  // Where the words are: mites keep out of these (and fade if they end up inside one).
  const WORDS = ".hero .eyebrow, .hero h1, .hero .lede, .hero .cta, .hero .fine, h2, .sub, .open p";
  let blocks = [], blocksAt = -1;
  function wordBlocks() {
    if (blocksAt === scrollY) return blocks;
    blocksAt = scrollY;
    blocks = [];
    for (const el of document.querySelectorAll(WORDS)) {
      const r = el.getBoundingClientRect();
      if (r.bottom < -40 || r.top > H + 40 || !r.width) continue;
      blocks.push(r);
    }
    return blocks;
  }
  const PAD = 18;
  function inside(m) {
    let depth = 0, ax = 0, ay = 0;
    for (const r of wordBlocks()) {
      const dx = Math.min(m.x - (r.left - PAD), r.right + PAD - m.x);
      const dy = Math.min(m.y - (r.top - PAD), r.bottom + PAD - m.y);
      if (dx <= 0 || dy <= 0) continue;
      const d = Math.min(dx, dy);
      if (d > depth) {
        depth = d;
        // Out by the nearest edge.
        const ex = [r.left - PAD - m.x, r.right + PAD - m.x], ey = [r.top - PAD - m.y, r.bottom + PAD - m.y];
        const bx = Math.abs(ex[0]) < Math.abs(ex[1]) ? ex[0] : ex[1], by = Math.abs(ey[0]) < Math.abs(ey[1]) ? ey[0] : ey[1];
        if (Math.abs(bx) < Math.abs(by)) { ax = Math.sign(bx); ay = 0; } else { ax = 0; ay = Math.sign(by); }
      }
    }
    return { depth, ax, ay };
  }

  let t0 = performance.now(), time = 0;
  function frame(now) {
    const dt = Math.min(0.05, (now - t0) / 1000);
    t0 = now; time += dt;
    // Scroll speed, smoothed: the page moving is what wakes the mites.
    const dy = scrollY - lastY; lastY = scrollY;
    velocity += (dy / Math.max(dt, 1e-3) - velocity) * Math.min(1, dt * 10);
    const excite = Math.min(1, Math.abs(velocity) / 1800);

    if (sig.getBoundingClientRect().bottom > 0) drawSignal(time, excite);

    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, cv.width, cv.height);
    if (burst) burst.t += dt;
    for (const m of mites) {
      // Wander: walk a while, rest a while; scrolling cancels the rest and sets them running.
      if (m.rest > 0 && excite < 0.05) { m.rest -= dt; } else if ((m.walk -= dt) < 0) { m.walk = rnd(2, 7); m.rest = rnd(0.3, 2.2); }
      let target = m.rest > 0 && excite < 0.05 ? 0 : m.base * (1 + excite * 5);
      m.turn += (rnd(-1, 1) * 3 - m.turn) * dt * 2;
      let steer = m.turn;
      // Scrolling: run with the page (content moves up when scrolling down).
      if (excite > 0.05) {
        const want = velocity > 0 ? -Math.PI / 2 : Math.PI / 2;
        steer += Math.sin(want - m.h) * 4 * excite;
      }
      // Flee the pointer and any click.
      for (const p of [pointer, burst && burst.t < 0.8 ? burst : null]) {
        if (!p) continue;
        const ex = m.x - p.x, ey = m.y - p.y, d = Math.hypot(ex, ey), r = p === burst ? 260 : 110;
        if (d < r) {
          steer += Math.sin(Math.atan2(ey, ex) - m.h) * 9;
          target = Math.max(target, m.base * 5 * (1 - d / r) + m.base);
          m.rest = 0;
        }
      }
      // Words: look ahead and turn away before reaching a text block; already inside, head out.
      const ahead = inside({ x: m.x + Math.cos(m.h) * m.size, y: m.y + Math.sin(m.h) * m.size });
      const here = inside(m);
      if (here.depth > 0) {
        steer += Math.sin(Math.atan2(here.ay, here.ax) - m.h) * 10;
        target = Math.max(target, m.base * 2);
        m.rest = 0;
      } else if (ahead.depth > 0) {
        steer += Math.sin(Math.atan2(ahead.ay, ahead.ax) - m.h) * 8;
      }
      m.fade += ((here.depth > 0 ? 0 : 1) - m.fade) * Math.min(1, dt * (here.depth > 0 ? 8 : 3));
      m.h += steer * dt;
      m.speed += (target - m.speed) * Math.min(1, dt * 6);
      const step = m.speed * dt;
      m.x += Math.cos(m.h) * step;
      m.y += Math.sin(m.h) * step - dy * m.depth * 0.6;
      m.gait += step / (m.size * 0.11);
      if (m.x < -60 || m.x > W + 60 || m.y < -60 || m.y > H + 60) spawn(m, true);
      drawMite(m);
    }
    ctx.globalAlpha = 1;
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
})();
