import {useLayoutEffect, useRef} from 'react';

/**
 * Shared sliding highlight for tab rows whose items have different widths (#1.20 layoutId).
 * Measures the `[aria-selected="true"]` child and writes its box to `--pill-x/y/w/h` on the track;
 * a `.fs-slide-pill` child renders the highlight from those vars. The first placement skips the
 * transition (`data-pill="on"`) so the pill never slides in from the corner on mount.
 */
export function useSlidingPill<T extends HTMLElement>(activeKey: unknown) {
  const ref = useRef<T>(null);
  useLayoutEffect(() => {
    const track = ref.current;
    if (!track) return;
    const place = () => {
      const active = track.querySelector<HTMLElement>('[aria-selected="true"]');
      if (!active) {
        delete track.dataset.pill;
        return;
      }
      track.style.setProperty('--pill-x', `${active.offsetLeft}px`);
      track.style.setProperty('--pill-y', `${active.offsetTop}px`);
      track.style.setProperty('--pill-w', `${active.offsetWidth}px`);
      track.style.setProperty('--pill-h', `${active.offsetHeight}px`);
      if (!track.dataset.pill) {
        track.dataset.pill = 'on';
        requestAnimationFrame(() => {
          if (track.dataset.pill) track.dataset.pill = 'ready';
        });
      }
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(track);
    return () => observer.disconnect();
  }, [activeKey]);
  return ref;
}
