/* STYLE_GUIDE · style-guide.js
   Behaviour for STYLE_GUIDE.html. A classic script on purpose, so the page works when opened from disk.
   1  Signal colour control        6  The approval gate mockup (M4)
   2  Navigation + scroll-spy      7  The live specimen (Appendix A)
   3  Reading progress             8  The live motion lab loader (Appendix C)
   4  Copy and download buttons    9  Hash targets inside closed <details>
   5  Mockup scaling and controls
*/
(function () {
  'use strict';

  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const root = document.documentElement;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const safe = (fn) => { try { return fn(); } catch (e) { return undefined; } };
  const topH = () => $('#sg-top').offsetHeight;

  /* ------------------------------------------------------------ 1 · signal colour */
  const SIGNAL_KEY = 'octran-style-guide-signal';
  const SIGNALS = ['#FF5E1A', '#FDC9A5', '#B9480F'];
  let signal = '#FF5E1A';
  function setSignal(hex) {
    signal = hex;
    root.style.setProperty('--sg-signal', hex);
    const lab = $('#lab-root');
    if (lab) lab.style.setProperty('--signal', hex);
    $$('.sg-tweak__dot').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.signal === hex)));
    safe(() => localStorage.setItem(SIGNAL_KEY, hex));
    if (window.OctranLab && window.OctranLab.setSignal) window.OctranLab.setSignal(hex);
  }
  $$('.sg-tweak__dot').forEach((b) => b.addEventListener('click', () => setSignal(b.dataset.signal)));
  const stored = safe(() => localStorage.getItem(SIGNAL_KEY));
  if (stored && SIGNALS.includes(stored) && stored !== signal) setSignal(stored);

  /* ------------------------------------------------------------ 2 · navigation */
  const nav = $('#sg-nav');
  const menu = $('.sg-top__menu');
  const setMenu = (open) => {
    nav.classList.toggle('is-open', open);
    menu.setAttribute('aria-expanded', String(open));
  };
  menu.addEventListener('click', () => setMenu(!nav.classList.contains('is-open')));
  nav.addEventListener('click', (e) => { if (e.target.closest('a') && matchMedia('(max-width:1024px)').matches) setMenu(false); });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') setMenu(false);
    if (e.key === '/' && !/^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName) && !document.activeElement.isContentEditable) {
      e.preventDefault();
      setMenu(true);
      $('#sg-find').focus();
    }
  });

  // filter
  const find = $('#sg-find');
  find.addEventListener('input', () => {
    const q = find.value.trim().toLowerCase();
    $$('.sg-nav__part').forEach((part) => {
      const link = $('.sg-nav__link', part);
      const partMatch = !q || link.textContent.toLowerCase().includes(q);
      let anySub = false;
      $$('.sg-nav__subs li', part).forEach((li) => {
        const m = !q || partMatch || li.textContent.toLowerCase().includes(q);
        li.hidden = !m;
        if (m && q) anySub = true;
      });
      part.hidden = !!q && !partMatch && !anySub;
      if (q) part.classList.add('is-open');
    });
    $$('.sg-nav__group').forEach((g) => { g.hidden = $$('.sg-nav__part', g).every((p) => p.hidden); });
    if (!q) spy(true);
  });

  // scroll-spy
  const spyEls = $$('[data-spy]').filter((el) => el.id);
  const linkById = new Map();
  $$('.sg-nav a[href^="#"]').forEach((a) => linkById.set(a.getAttribute('href').slice(1), a));
  let lastActive = '';
  function spy(force) {
    const line = topH() + 90;
    let active = null;
    let part = null;
    for (const el of spyEls) {
      if (el.getBoundingClientRect().top <= line) {
        active = el;
        if (el.tagName === 'H2' || el.id === 'start') part = el;
      } else break;
    }
    const id = active ? active.id : 'start';
    if (!force && id === lastActive) return;
    lastActive = id;
    $$('.sg-nav a.is-active').forEach((a) => a.classList.remove('is-active'));
    $$('.sg-nav__part.is-open').forEach((p) => { if (!find.value) p.classList.remove('is-open'); });
    const link = linkById.get(id);
    const partLink = part && linkById.get(part.id);
    if (partLink) partLink.classList.add('is-active');
    if (link) {
      if (link !== partLink) link.classList.add('is-active');
      const box = link.closest('.sg-nav__part');
      if (box) box.classList.add('is-open');
      const nr = nav.getBoundingClientRect();
      const lr = link.getBoundingClientRect();
      if (lr.top < nr.top + 40 || lr.bottom > nr.bottom - 40) nav.scrollTop += lr.top - nr.top - nr.height / 2;
    }
  }

  /* ------------------------------------------------------------ 3 · progress + scheduling */
  const progress = $('.sg-progress');
  let ticking = false;
  function onScroll() {
    if (ticking) return;
    ticking = true;
    requestAnimationFrame(() => {
      ticking = false;
      const max = root.scrollHeight - innerHeight;
      progress.style.setProperty('--p', max > 0 ? Math.min(1, scrollY / max).toFixed(4) : 0);
      spy(false);
      updateFill();
    });
  }
  addEventListener('scroll', onScroll, { passive: true });

  /* ------------------------------------------------------------ 4 · copy + download */
  document.addEventListener('click', (e) => {
    const copy = e.target.closest('[data-copy]');
    const dl = e.target.closest('[data-download]');
    if (copy) {
      const text = $(copy.dataset.copy).textContent;
      const done = () => {
        const old = copy.textContent;
        copy.textContent = 'Copied';
        setTimeout(() => { copy.textContent = old; }, 1500);
      };
      if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(text).then(done, () => fallbackCopy(text, done));
      else fallbackCopy(text, done);
    }
    if (dl) {
      const blob = new Blob([$(dl.dataset.download).textContent], { type: dl.dataset.type || 'text/plain' });
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = dl.dataset.filename || 'download.txt';
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    }
  });
  function fallbackCopy(text, done) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.cssText = 'position:fixed;opacity:0';
    document.body.appendChild(ta);
    ta.select();
    safe(() => document.execCommand('copy'));
    ta.remove();
    done();
  }

  /* ------------------------------------------------------------ 5 · mockup scaling */
  const figures = $$('.sg-ab');
  function layout(fig) {
    const view = $('.sg-ab__view', fig);
    const stage = $('.sg-ab__stage', fig);
    const full = fig.classList.contains('is-full');
    const w = Number(fig.dataset.currentW || fig.dataset.w);
    stage.style.width = w + 'px';
    const avail = view.clientWidth;
    if (!avail) return;
    const s = full ? 1 : Math.min(1, avail / w);
    const h = stage.offsetHeight || Number(fig.dataset.h) || 0;
    const dx = full ? 0 : Math.max(0, (avail - w * s) / 2);
    stage.style.transform = s === 1 && !dx ? 'none' : `translate(${dx}px, 0) scale(${s})`;
    fig.classList.add('is-scaled');
    view.style.height = full ? '' : Math.ceil(h * s) + 'px';
    fig.style.setProperty('--ab-scale', s.toFixed(3));
    const meta = $('.sg-ab__meta', fig);
    if (meta) meta.dataset.scale = Math.round(s * 100) + '%';
  }
  const layoutAll = () => figures.forEach(layout);
  const ro = new ResizeObserver((entries) => {
    entries.forEach((en) => { const fig = en.target.closest('.sg-ab'); if (fig) layout(fig); });
  });
  figures.forEach((fig) => {
    ro.observe($('.sg-ab__view', fig));
    ro.observe($('.sg-ab__stage', fig));
  });
  figures.forEach((fig) => {
    const stage = $('.sg-ab__stage', fig);
    $$('[data-ab-width]', fig).forEach((b) => b.addEventListener('click', () => {
      const w = Number(b.dataset.abWidth);
      fig.dataset.currentW = w;
      stage.classList.toggle('is-narrow', w <= 760);
      $$('[data-ab-width]', fig).forEach((o) => o.setAttribute('aria-pressed', String(o === b)));
      layout(fig);
    }));
    const full = $('[data-ab-full]', fig);
    if (full) full.addEventListener('click', () => {
      const on = fig.classList.toggle('is-full');
      full.setAttribute('aria-pressed', String(on));
      full.textContent = on ? 'Fit to width' : 'Full size';
      layout(fig);
    });
  });
  addEventListener('load', () => { layoutAll(); spy(true); });
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(layoutAll);
  layoutAll();

  // Links inside mockups and the specimen are illustrations, not navigation.
  document.addEventListener('click', (e) => {
    const a = e.target.closest('.sg-ab a[href="#"], .sg-ab a[data-site-route]');
    if (a) e.preventDefault();
  });

  /* ------------------------------------------------------------ 6 · approval gate (M4) */
  const gate = $('#m4');
  if (gate) {
    const looks = {
      hold: { bar: 0, packet: 760, needle: 0, label: 'HOLD · WAITING FOR APPROVAL', stamp: 'OPEN — WAITING FOR THE ACCOUNT OWNER' },
      approve: { bar: -68, packet: 1150, needle: 58, label: 'APPROVED · ON TO LEDGER', stamp: 'APPROVED — REPORTED SENT (ILLUSTRATIVE)' },
      decline: { bar: 0, packet: 706, needle: -58, label: 'DECLINED · RETURNED', stamp: 'NOT SENT — RETURNED TO THE OWNER' },
    };
    const q = (n) => $(`[data-gate="${n}"]`, gate);
    const setGate = (mode) => {
      const l = looks[mode];
      q('bar').style.transform = `rotate(${l.bar}deg)`;
      q('packet').style.transform = `translate(${l.packet}px, 0px)`;
      q('needle').style.transform = `rotate(${l.needle}deg)`;
      q('state').textContent = l.label;
      q('state').style.color = mode === 'hold' ? '#FDC9A5' : mode === 'approve' ? 'var(--sg-signal)' : '#B4B7BE';
      q('hold').setAttribute('fill', mode === 'hold' ? '#F4F4F5' : '#8A8F98');
      q('approve-label').setAttribute('fill', mode === 'approve' ? 'var(--sg-signal)' : '#8A8F98');
      q('decline-label').setAttribute('fill', mode === 'decline' ? '#F4F4F5' : '#8A8F98');
      q('stamp').textContent = l.stamp;
    };
    $$('[data-gate-action]', gate).forEach((b) => b.addEventListener('click', () => setGate(b.dataset.gateAction === 'reset' ? 'hold' : b.dataset.gateAction)));
  }

  /* ------------------------------------------------------------ 7 · live specimen */
  const spec = $('#specimen');
  let updateFill = () => {};
  if (spec) {
    const stageCopy = [
      ['Find the relevant sources.', 'Connect the original question to the records and people who can help.'],
      ['A prepared step waits for review.', 'Nexus applies the agreed permissions; an authorized person decides whether the action proceeds.'],
      ['Keep the result with the task.', 'Ledger records what happened. Failed or unresolved work stays visible.'],
    ];
    $$('[data-stage]', spec).forEach((button) => button.addEventListener('click', () => {
      $$('[data-stage]', spec).forEach((item) => item.setAttribute('aria-pressed', String(item === button)));
      const [title, copy] = stageCopy[Number(button.dataset.stage)];
      const box = $('.stage-copy', spec);
      box.textContent = '';
      const strong = document.createElement('strong');
      strong.textContent = title;
      const span = document.createElement('span');
      span.textContent = copy;
      box.append(strong, span);
    }));

    const demo = $('#sp-motion-demo', spec);
    if (demo) demo.querySelector('button').addEventListener('click', (e) => {
      const on = demo.classList.toggle('on');
      e.currentTarget.setAttribute('aria-pressed', String(on));
    });

    const inner = $('.specimen-inner', spec);
    if (reduced || !('IntersectionObserver' in window)) {
      inner.classList.remove('reveal-on');
    } else {
      const io = new IntersectionObserver((entries) => entries.forEach((en) => {
        if (en.isIntersecting) { en.target.classList.add('in'); io.unobserve(en.target); }
      }), { threshold: 0.35 });
      $$('.map, .feed', spec).forEach((el) => io.observe(el));
    }

    const fill = $('#sp-fill-demo', spec);
    if (fill) {
      updateFill = () => {
        const r = fill.getBoundingClientRect();
        const p = reduced ? 1 : Math.min(1, Math.max(0, (innerHeight - r.top) / (innerHeight * 0.55)));
        fill.style.setProperty('--fill', p.toFixed(3));
        fill.classList.toggle('is-filled', p > 0.8);
      };
      updateFill();
      addEventListener('resize', updateFill);
    }
  }

  /* ------------------------------------------------------------ 8 · live motion lab */
  const lab = $('#lab-root');
  if (lab) {
    const status = $('#lab-status');
    const fail = (err) => {
      if (status) {
        status.hidden = false;
        status.textContent = 'The live lab could not start. It needs a modern browser and network access to cdn.jsdelivr.net for three.js and anime.js. Mockup M2 shows the same scene as a still.';
      }
      if (window.console) console.warn('[style guide] live lab failed', err);
    };
    let started = false;
    const start = () => {
      if (started) return;
      started = true;
      const script = document.createElement('script');
      script.src = 'style-guide/lab.js';
      script.onload = () => {
        Promise.resolve(window.OctranLab.start())
          .then(() => {
            if (status) status.hidden = true;
            window.OctranLab.setSignal(signal);
          })
          .catch(fail);
      };
      script.onerror = fail;
      document.head.appendChild(script);
    };
    if ('IntersectionObserver' in window) {
      const io = new IntersectionObserver((entries) => {
        if (entries.some((en) => en.isIntersecting)) { io.disconnect(); start(); }
      }, { rootMargin: '700px 0px' });
      io.observe(lab);
    } else start();
  }

  /* ------------------------------------------------------------ 9 · hash targets in <details> */
  function openFor(hash) {
    if (!hash || hash.length < 2) return;
    const el = safe(() => document.getElementById(decodeURIComponent(hash.slice(1))));
    if (!el) return;
    const d = el.closest('details');
    if (d && !d.open) {
      d.open = true;
      requestAnimationFrame(() => el.scrollIntoView());
    }
  }
  addEventListener('hashchange', () => openFor(location.hash));
  openFor(location.hash);
  onScroll();
})();
