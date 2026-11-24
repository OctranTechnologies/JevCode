// Octran live motion lab: blueprint renderer + anime.js, used by STYLE_GUIDE.html (Appendix C).
// A classic script on purpose: it uses dynamic import() so the page also works when opened from disk.
// three.js and anime.js come from the import map in STYLE_GUIDE.html (cdn.jsdelivr.net).
(function () {
  'use strict';
  window.OctranLab = window.OctranLab || {};

  async function boot() {
    const THREE = await import('three');
    const { animate, createTimeline, createTimer, createSpring, onScroll, stagger, scrambleText, utils } = await import('animejs');
    await import('animejs/adapters/three');

    // Octran motion lab: "Ink & Signal", blueprint edition.
    // anime.js 4.5 drives every value (DOM, SVG and Three.js through its official adapter);
    // Three.js renders procedural parts as line-only blueprint schematics: two passes, no fills.
    THREE.ColorManagement.enabled = false;

    const LAB = document.getElementById('lab-root');
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    const css = getComputedStyle(LAB);
    const token = (name) => new THREE.Color(css.getPropertyValue(name).trim());

    // ---------------------------------------------------------------- renderer

    const canvas = document.getElementById('lab-engine');
    const leaders = document.getElementById('lab-leaders');
    const renderer = new THREE.WebGLRenderer({ canvas, antialias: false, alpha: true, powerPreference: 'high-performance' });
    renderer.outputColorSpace = THREE.LinearSRGBColorSpace;
    renderer.autoClear = false;

    const scene = new THREE.Scene();
    const gridScene = new THREE.Scene();
    const quadScene = new THREE.Scene();
    const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 100);
    camera.position.set(10, 10, 10);
    camera.lookAt(0, 0.4, 0);
    const encoded = new THREE.WebGLRenderTarget(1, 1, { depthBuffer: true });

    // Pass 1: each part writes an ID colour mixed with its faceted view normal (RGB) and a signal flag (A).
    const encodeVertex = /* glsl */ `
      varying vec3 vViewPos;
      void main() {
        vec4 mv = modelViewMatrix * vec4(position, 1.0);
        vViewPos = mv.xyz;
        gl_Position = projectionMatrix * mv;
      }`;
    const encodeFragment = /* glsl */ `
      uniform vec3 uId;
      uniform float uSignal;
      varying vec3 vViewPos;
      void main() {
        vec3 n = normalize(cross(dFdx(vViewPos), dFdy(vViewPos)));
        gl_FragColor = vec4(mix(uId, n * .5 + .5, .25), uSignal > .5 ? .8 : .5);
      }`;

    let partCount = 0;
    function part(geometry, { signal = false } = {}) {
      const id = new THREE.Color().setHSL((partCount++ * 0.618034) % 1, 0.75, 0.5);
      const material = new THREE.ShaderMaterial({
        vertexShader: encodeVertex,
        fragmentShader: encodeFragment,
        uniforms: { uId: { value: id }, uSignal: { value: signal ? 1 : 0 } },
      });
      return new THREE.Mesh(geometry, material);
    }

    // Pass 2: blueprint lines. Colour jumps (part seams and creases sharper than ~33°) become lines;
    // part interiors are painted with the ground colour so hidden lines stay hidden. No fills, no shading.
    const composite = new THREE.ShaderMaterial({
      transparent: true,
      premultipliedAlpha: true,
      depthTest: false,
      depthWrite: false,
      uniforms: {
        tMap: { value: encoded.texture },
        uTexel: { value: new THREE.Vector2(1, 1) },
        uThreshold: { value: 0.14 },
        uGround: { value: token('--ground') },
        uLine: { value: token('--line') },
        uSignal: { value: token('--signal') },
      },
      vertexShader: /* glsl */ `
        varying vec2 vUv;
        void main() { vUv = uv; gl_Position = vec4(position.xy, 0.0, 1.0); }`,
      fragmentShader: /* glsl */ `
        uniform sampler2D tMap;
        uniform vec2 uTexel;
        uniform float uThreshold;
        uniform vec3 uGround, uLine, uSignal;
        varying vec2 vUv;
        float isSignal(vec4 v) { return step(.7, v.a) * step(v.a, .9); }
        void main() {
          vec4 s = texture2D(tMap, vUv);
          vec4 a = texture2D(tMap, vUv - uTexel);
          vec4 b = texture2D(tMap, vUv + uTexel);
          vec4 c = texture2D(tMap, vUv + vec2(uTexel.x, -uTexel.y));
          vec4 d = texture2D(tMap, vUv + vec2(-uTexel.x, uTexel.y));
          float edge = length(b.rgb - a.rgb) + length(d.rgb - c.rgb);
          float line = smoothstep(uThreshold, uThreshold * 1.6, edge);
          float signal = max(max(isSignal(s), isSignal(a)), max(max(isSignal(b), isSignal(c)), isSignal(d)));
          vec3 stroke = mix(uLine, uSignal, signal);
          if (s.a < .02) {
            gl_FragColor = vec4(stroke * line, line);
            return;
          }
          gl_FragColor = vec4(mix(uGround, stroke, line), 1.0);
        }`,
    });
    const quad = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), composite);
    quad.frustumCulled = false;
    quadScene.add(quad);
    window.OctranLab.setSignal = (hex) => composite.uniforms.uSignal.value.set(hex);

    const grid = new THREE.GridHelper(18, 18, 0x8a8f98, 0x8a8f98);
    grid.material.transparent = true;
    grid.material.opacity = 0.16;
    grid.material.depthWrite = false;
    gridScene.add(grid);

    // ---------------------------------------------------------------- parts

    const root = new THREE.Group();
    scene.add(root);
    const core = new THREE.Group();
    root.add(core);

    const coreBody = part(new THREE.CylinderGeometry(1.6, 1.6, 0.62, 8));
    coreBody.rotation.y = Math.PI / 8;
    coreBody.position.y = 0.31;
    const corePanel = part(new THREE.CylinderGeometry(0.95, 0.95, 0.06, 8));
    corePanel.rotation.y = Math.PI / 8;
    corePanel.position.y = 0.64;
    core.add(coreBody, corePanel);

    // The gem holder takes scroll-driven scale; the gem itself takes the intro and the idle spin.
    const gemHolder = new THREE.Group();
    gemHolder.position.y = 1.05;
    const gem = part(new THREE.OctahedronGeometry(0.26), { signal: true });
    gemHolder.add(gem);
    core.add(gemHolder);

    const armGeometry = new THREE.BoxGeometry(1.3, 0.22, 0.3);
    const pinGeometry = new THREE.CylinderGeometry(0.09, 0.09, 0.26, 8);
    const armPivots = [];
    const arms = [];
    for (let i = 0; i < 8; i++) {
      const pivot = new THREE.Group();
      pivot.rotation.y = (i * Math.PI) / 4;
      const arm = part(armGeometry);
      arm.position.set(2.6, 0.29, 0);
      const pin = part(pinGeometry);
      pin.position.set(-0.5, 0.12, 0);
      arm.add(pin);
      pivot.add(arm);
      core.add(pivot);
      armPivots.push(pivot);
      arms.push(arm);
    }

    // Arm 6 points at world +z (front-left in the isometric view) and carries the signal; arm 7 points at the viewer.
    const signalGeometry = new THREE.BoxGeometry(1.3, 0.03, 0.1);
    signalGeometry.translate(-0.65, 0, 0);
    const signalStrip = part(signalGeometry, { signal: true });
    signalStrip.position.set(0.65, 0.125, 0);
    signalStrip.scale.x = 0.0001;
    arms[6].add(signalStrip);
    const bead = part(new THREE.BoxGeometry(0.16, 0.16, 0.16), { signal: true });
    bead.position.set(0, 0.45, 3.25);
    bead.scale.setScalar(0.0001);
    core.add(bead);

    const modules = {};
    function moduleGroup(name, dock) {
      const group = new THREE.Group();
      const from = new THREE.Vector3(dock.x * 1.7, dock.y + 2.3, dock.z * 1.7);
      group.userData = { dock, from };
      group.position.copy(from);
      root.add(group);
      modules[name] = group;
      return group;
    }

    const tele = moduleGroup('tele', new THREE.Vector3(-0.2, 0.67, 0.95));
    const drum = part(new THREE.CylinderGeometry(0.4, 0.4, 0.44, 14));
    drum.position.y = 0.22;
    tele.add(drum);
    const barGeometry = new THREE.BoxGeometry(0.05, 0.3, 0.05);
    barGeometry.translate(0, 0.15, 0);
    const bars = [];
    for (let i = 0; i < 9; i++) {
      const bar = part(barGeometry, { signal: i > 2 && i < 6 });
      bar.position.set((i - 4) * 0.075, 0.44, 0);
      tele.add(bar);
      bars.push(bar);
    }

    const cortex = moduleGroup('cortex', new THREE.Vector3(-0.8, 0.67, -0.8));
    const lattice = new THREE.Group();
    lattice.position.y = 0.32;
    cortex.add(lattice);
    const nodeGeometry = new THREE.BoxGeometry(0.12, 0.12, 0.12);
    const nodes = [];
    for (let x = -1; x <= 1; x++) for (let y = -1; y <= 1; y++) for (let z = -1; z <= 1; z++) {
      const node = part(nodeGeometry, { signal: x === 0 && y === 0 && z === 0 });
      node.position.set(x * 0.24, y * 0.24, z * 0.24);
      lattice.add(node);
      nodes.push(node);
    }

    const ledger = moduleGroup('ledger', new THREE.Vector3(0.95, 0.67, -0.2));
    const flangeGeometry = new THREE.CylinderGeometry(0.42, 0.42, 0.06, 14);
    const lowerFlange = part(flangeGeometry);
    lowerFlange.position.y = 0.03;
    const hub = part(new THREE.CylinderGeometry(0.22, 0.22, 0.3, 12));
    hub.position.y = 0.21;
    const upperFlange = part(flangeGeometry);
    upperFlange.position.y = 0.39;
    const tapeGeometry = new THREE.BoxGeometry(1, 0.02, 0.22);
    tapeGeometry.translate(0.5, 0, 0);
    const tape = part(tapeGeometry);
    tape.position.set(0.22, 0.2, 0);
    tape.scale.x = 0.0001;
    ledger.add(lowerFlange, hub, upperFlange, tape);

    // ---------------------------------------------------------------- size

    const VIEW_HEIGHT = 8.5;
    let width = 1;
    let height = 1;
    function resize() {
      width = canvas.clientWidth;
      height = canvas.clientHeight;
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      renderer.setPixelRatio(dpr);
      renderer.setSize(width, height, false);
      encoded.setSize(Math.round(width * dpr), Math.round(height * dpr));
      composite.uniforms.uTexel.value.set(1 / (width * dpr), 1 / (height * dpr));
      // Wide screens push the model right of the headline; narrower ones drop it below the copy.
      const compact = width < 1024;
      const aspect = width / height;
      const viewHeight = compact ? Math.max(12, 9 / aspect) : VIEW_HEIGHT;
      const viewWidth = viewHeight * aspect;
      const shift = compact ? 0 : viewWidth * 0.2;
      camera.left = -viewWidth / 2 - shift;
      camera.right = viewWidth / 2 - shift;
      camera.top = compact ? viewHeight * 0.72 : viewHeight / 2;
      camera.bottom = compact ? -viewHeight * 0.28 : -viewHeight / 2;
      camera.updateProjectionMatrix();
      leaders.setAttribute('viewBox', `0 0 ${width} ${height}`);
    }
    new ResizeObserver(resize).observe(canvas);
    resize();

    // ---------------------------------------------------------------- callouts

    const callouts = [...LAB.querySelectorAll('.callout')];
    const calloutOf = (name) => callouts.find((el) => el.dataset.anchor === name);
    const anchorOf = {
      tele: { object: tele, offset: new THREE.Vector3(0, 0.75, 0), dx: -150, dy: -50 },
      cortex: { object: cortex, offset: new THREE.Vector3(0, 0.75, 0), dx: -130, dy: -90 },
      ledger: { object: ledger, offset: new THREE.Vector3(0, 0.45, 0), dx: 110, dy: -80 },
      tako: { object: arms[7], offset: new THREE.Vector3(0.65, 0.12, 0), dx: 90, dy: 40 },
    };
    leaders.innerHTML = callouts.map(() => '<polyline points=""></polyline><circle r="3"></circle>').join('');
    const leaderLines = [...leaders.querySelectorAll('polyline')];
    const leaderDots = [...leaders.querySelectorAll('circle')];
    const point = new THREE.Vector3();

    function updateCallouts() {
      callouts.forEach((el, i) => {
        const a = anchorOf[el.dataset.anchor];
        a.object.updateWorldMatrix(true, false);
        point.copy(a.offset).applyMatrix4(a.object.matrixWorld).project(camera);
        const x = ((point.x + 1) / 2) * width;
        const y = ((1 - point.y) / 2) * height;
        const ex = x + a.dx;
        const ey = y + a.dy;
        const run = 118;
        const end = a.dx < 0 ? ex - run : ex + run;
        el.style.transform = `translate(${Math.round(a.dx < 0 ? end : ex)}px, ${Math.round(ey - 19)}px)`;
        leaderLines[i].setAttribute('points', `${x},${y} ${ex},${ey} ${end},${ey}`);
        leaderDots[i].setAttribute('cx', x);
        leaderDots[i].setAttribute('cy', y);
        const opacity = el.style.opacity || '0';
        leaderLines[i].style.opacity = opacity;
        leaderDots[i].style.opacity = opacity;
      });
    }

    // ---------------------------------------------------------------- render loop

    function render() {
      renderer.setRenderTarget(encoded);
      renderer.setClearColor(0xffffff, 0);
      renderer.clear();
      renderer.render(scene, camera);
      renderer.setRenderTarget(null);
      renderer.setClearColor(0x000000, 0);
      renderer.clear();
      renderer.render(gridScene, camera);
      renderer.render(quadScene, camera);
      updateCallouts();
    }
    const renderLoop = createTimer({ onUpdate: render });
    new IntersectionObserver(([entry]) => (entry.isIntersecting ? renderLoop.play() : renderLoop.pause()))
      .observe(LAB.querySelector('.stage'));

    // ---------------------------------------------------------------- intro and idle motion

    if (reduced) {
      render();
    } else {
      createTimeline({ defaults: { ease: 'out(3)' } })
        .add(core, { y: [-1.2, 0], duration: 900 })
        .add(arms, { x: [3.6, 2.6], scale: [0, 1], duration: 700, delay: stagger(60, { from: 'center' }) }, 300)
        .add(gem, { scale: [0, 1], duration: 800 }, 600);

      animate(gem, { rotateY: '+=360', duration: 8000, ease: 'linear', loop: true });
      animate(drum, { rotateY: '+=360', duration: 6000, ease: 'linear', loop: true });
      animate(bars, { scaleY: [0.3, 1], duration: 420, ease: 'inOut(3)', loop: true, alternate: true, delay: stagger(70, { from: 'center' }) });
      animate(nodes, { scale: [1, 1.45], duration: 600, ease: 'inOut(2)', loop: true, alternate: true, delay: stagger(40, { from: 'center' }) });
      animate(lattice, { rotateY: '+=360', duration: 16000, ease: 'linear', loop: true });

      const words = ['order', 'invoice', 'reply', 'handover', 'work'];
      let word = words.length - 1;
      createTimer({
        duration: 2400,
        loop: true,
        onLoop: () => {
          word = (word + 1) % words.length;
          animate('#lab-cycle', { innerHTML: scrambleText({ text: words[word], chars: 'blocks' }), duration: 700 });
        },
      });
    }

    // ---------------------------------------------------------------- scroll timeline

    const chapters = [
      { at: 0, code: '00 / CORE', text: 'Scroll to dock the three foundations into the core.' },
      { at: 0.12, code: '01 / DOCK', text: 'Tele, Cortex and Ledger seat into the core: conversations, understanding and work history in one place.' },
      { at: 0.25, code: '02 / SIGNAL', text: 'A customer question travels an arm into the core. Orange marks where the work is moving.' },
      { at: 0.52, code: '03 / TAKO', text: 'Tako’s eight arms turn to prepare the next step, within the permissions your company sets.' },
      { at: 0.75, code: '04 / LEDGER', text: 'Ledger prints what happened, including what failed and what is still open.' },
    ];
    const chapterCode = document.getElementById('lab-chapter-code');
    const chapterText = document.getElementById('lab-chapter-text');
    let chapterIndex = 0;

    const dial = document.getElementById('lab-dial');
    const segmentPaths = ['M38.6,8.3 L81.4,8.3 L75.3,23.0 L44.7,23.0 Z', 'M81.4,8.3 L111.7,38.6 L97.0,44.7 L75.3,23.0 Z', 'M111.7,38.6 L111.7,81.4 L97.0,75.3 L97.0,44.7 Z', 'M111.7,81.4 L81.4,111.7 L75.3,97.0 L97.0,75.3 Z', 'M81.4,111.7 L38.6,111.7 L44.7,97.0 L75.3,97.0 Z', 'M38.6,111.7 L8.3,81.4 L23.0,75.3 L44.7,97.0 Z', 'M8.3,81.4 L8.3,38.6 L23.0,44.7 L23.0,75.3 Z', 'M8.3,38.6 L38.6,8.3 L44.7,23.0 L23.0,44.7 Z'];
    const segmentGroup = dial.querySelector('.segments');
    segmentGroup.innerHTML = segmentPaths.map((d) => `<path d="${d}"></path>`).join('');
    const segments = [...segmentGroup.children];
    const needle = dial.querySelector('.needle');

    function showProgress(progress) {
      const lit = Math.min(8, Math.floor(progress * 8 + 0.0001) + 1);
      segments.forEach((segment, i) => segment.classList.toggle('on', i < lit));
      needle.setAttribute('transform', `rotate(${progress * 360} 60 60)`);
      let next = 0;
      chapters.forEach((chapter, i) => { if (progress >= chapter.at) next = i; });
      dial.setAttribute('aria-valuenow', String(lit));
      dial.setAttribute('aria-valuetext', chapters[next].code.slice(5));
      if (next === chapterIndex) return;
      chapterIndex = next;
      chapterText.textContent = chapters[next].text;
      if (reduced) chapterCode.textContent = chapters[next].code;
      else animate(chapterCode, { innerHTML: scrambleText({ text: chapters[next].code, chars: 'blocks' }), duration: 450 });
    }

    const moduleList = [tele, cortex, ledger];
    const dockCallouts = ['tele', 'cortex', 'ledger'].map(calloutOf);
    const master = createTimeline({
      defaults: { ease: 'inOut(3)', duration: 1000 },
      autoplay: onScroll({ target: '#lab-track', enter: 'top top', leave: 'bottom bottom', sync: reduced ? true : 0.7 }),
      onUpdate: (self) => showProgress(self.progress),
    });

    master
      .add('#lab-headline', { opacity: [1, 0], y: [0, -40], duration: 500, ease: 'in(2)' }, 250)
      .add(moduleList, {
        x: (m) => [m.userData.from.x, m.userData.dock.x],
        y: (m) => [m.userData.from.y, m.userData.dock.y],
        z: (m) => [m.userData.from.z, m.userData.dock.z],
        rotateY: [-90, 0],
        delay: stagger(220),
      }, 450)
      .add(dockCallouts, { opacity: [0, 1], duration: 300, ease: 'out(3)', delay: stagger(220) }, 1300)
      .label('signal', 2000)
      .add(signalStrip, { scaleX: [0.0001, 1], duration: 700, ease: 'linear' }, 'signal')
      .add(bead, { scale: [0.0001, 1], duration: 150, ease: 'out(3)' }, 'signal+=650')
      .add(bead, { z: [3.25, 1.95], duration: 500, ease: 'linear' }, 'signal+=700')
      .add(bead, { z: [1.95, 0], y: [0.45, 1.05], duration: 500, ease: 'in(2)' }, 'signal+=1200')
      .add(bead, { scale: [1, 0.0001], duration: 200 }, 'signal+=1650')
      .add(gemHolder, { scale: [1, 1.7], duration: 400, ease: 'out(3)' }, 'signal+=1650')
      .label('tako', 4200)
      .add(armPivots, { rotateY: (p, i) => [i * 45, i * 45 + 22.5], duration: 900, delay: stagger(60, { from: 'center' }) }, 'tako')
      .add(arms, { rotateZ: [0, 14], duration: 600, ease: 'out(3)', delay: stagger(60, { from: 'center' }) }, 'tako+=300')
      .add(calloutOf('tako'), { opacity: [0, 1], duration: 300, ease: 'out(3)' }, 'tako+=700')
      .label('ledger', 6000)
      .add(tape, { scaleX: [0.0001, 1], duration: 900, ease: 'out(3)' }, 'ledger')
      .add(gemHolder, { scale: [1.7, 1], duration: 600 }, 'ledger')
      .add(lattice, { y: [0.32, 0.42], duration: 600, ease: 'out(3)' }, 'ledger')
      .add(dial, { scale: [1, 1], duration: 1 }, 8000);

    showProgress(0);

    // The dial seeks the page: drag around it, click a segment or use the arrow keys.
    const track = document.getElementById('lab-track');
    const trackTop = () => track.getBoundingClientRect().top + window.scrollY;
    function scrollToProgress(progress) {
      const p = Math.min(1, Math.max(0, progress));
      const top = trackTop() + p * (track.offsetHeight - window.innerHeight);
      window.scrollTo({ top, behavior: reduced ? 'auto' : 'smooth' });
    }
    function progressFromPointer(event) {
      const box = dial.getBoundingClientRect();
      const angle = Math.atan2(event.clientX - (box.left + box.width / 2), -(event.clientY - (box.top + box.height / 2)));
      return ((angle / (Math.PI * 2)) + 1) % 1;
    }
    let dragging = false;
    dial.addEventListener('pointerdown', (event) => {
      dragging = true;
      dial.setPointerCapture(event.pointerId);
      scrollToProgress(progressFromPointer(event));
    });
    dial.addEventListener('pointermove', (event) => {
      if (!dragging) return;
      const p = Math.min(1, Math.max(0, progressFromPointer(event)));
      window.scrollTo({ top: trackTop() + p * (track.offsetHeight - window.innerHeight), behavior: 'auto' });
    });
    dial.addEventListener('pointerup', () => { dragging = false; });
    dial.addEventListener('keydown', (event) => {
      const step = event.key === 'ArrowRight' || event.key === 'ArrowUp' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowDown' ? -1 : 0;
      if (!step) return;
      event.preventDefault();
      const current = Math.round(master.progress * 8);
      scrollToProgress((current + step) / 8);
    });

    // ---------------------------------------------------------------- gate

    const bar = document.getElementById('lab-bar');
    const packet = document.getElementById('lab-packet');
    const stamp = document.getElementById('lab-stamp');
    bar.style.transformOrigin = '220px 170px';
    utils.set(packet, { x: 140 });
    const gateStates = {
      approve: { rotate: -68, x: 556, stamp: 'APPROVED — REPORTED SENT (ILLUSTRATIVE)' },
      decline: { rotate: 0, x: 24, stamp: 'NOT SENT — RETURNED TO THE OWNER' },
      reset: { rotate: 0, x: 140, stamp: 'OPEN — WAITING FOR THE ACCOUNT OWNER' },
    };
    LAB.querySelectorAll('[data-lab-gate]').forEach((button) => {
      button.addEventListener('click', () => {
        const state = gateStates[button.dataset.labGate];
        if (reduced) {
          utils.set(bar, { rotate: state.rotate });
          utils.set(packet, { x: state.x });
          stamp.textContent = state.stamp;
          return;
        }
        animate(bar, { rotate: state.rotate, ease: createSpring({ bounce: 0.35, duration: 700 }) });
        animate(packet, { x: state.x, duration: 900, ease: 'inOut(3)', delay: state.rotate ? 250 : 0 });
        animate(stamp, { innerHTML: scrambleText({ text: state.stamp, chars: 'uppercase' }), duration: 700 });
      });
    });
  }

  window.OctranLab.start = boot;
})();
