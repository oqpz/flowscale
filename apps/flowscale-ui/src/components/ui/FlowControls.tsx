import {type CSSProperties, type ReactNode, useEffect, useRef, useState} from 'react';
import {createPortal} from 'react-dom';
import {CheckIcon, ChevronDownIcon, XMarkIcon} from '@heroicons/react/24/outline';

export function FlowButton({
  label,
  variant = 'secondary',
  size = 'md',
  icon,
  disabled,
  title,
  ariaLabel,
  className = '',
  onClick,
  type = 'button',
}: {
  label?: string;
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
  size?: 'sm' | 'md';
  icon?: ReactNode;
  disabled?: boolean;
  title?: string;
  ariaLabel?: string;
  className?: string;
  onClick?: () => void;
  type?: 'button' | 'submit';
}) {
  const variantClass =
    variant === 'primary'
      ? 'fs-btn-primary'
      : variant === 'ghost'
        ? 'fs-btn-ghost'
        : variant === 'danger'
          ? 'fs-btn-danger'
          : '';
  const sizeClass = !label && icon ? 'fs-btn-icon' : size === 'sm' ? 'fs-btn-sm' : '';
  return (
    <button
      type={type}
      className={`fs-btn ${variantClass} ${sizeClass} ${className}`.trim()}
      disabled={disabled}
      title={title}
      aria-label={ariaLabel ?? label}
      onClick={onClick}
    >
      {icon}
      {label && <span>{label}</span>}
    </button>
  );
}

export function FlowSwitch({
  label,
  description,
  checked,
  disabled,
  disabledMessage,
  onChange,
}: {
  label: string;
  description?: string;
  checked: boolean;
  disabled?: boolean;
  disabledMessage?: string;
  onChange: (checked: boolean) => void;
}) {
  const [stretching, setStretching] = useState(false);

  const triggerToggle = () => {
    if (disabled) return;
    setStretching(true);
    window.setTimeout(() => setStretching(false), 160);
    onChange(!checked);
  };

  return (
    <div
      role="switch"
      tabIndex={disabled ? -1 : 0}
      aria-checked={checked}
      aria-disabled={disabled || undefined}
      title={disabled ? disabledMessage : undefined}
      data-disabled={disabled ? 'true' : 'false'}
      className="fs-switch-row"
      onClick={triggerToggle}
      onKeyDown={(e) => {
        if (disabled) return;
        if (e.key === ' ' || e.key === 'Enter') {
          e.preventDefault();
          triggerToggle();
        }
      }}
    >
      <div style={{minWidth: 0, flex: 1}}>
        <div className="fs-field-label">{label}</div>
        {description && <div className="fs-field-desc">{description}</div>}
      </div>
      <div
        className="fs-switch-track"
        data-checked={checked ? 'true' : 'false'}
        data-stretching={stretching ? 'true' : 'false'}
      >
        <div className="fs-switch-thumb" />
      </div>
    </div>
  );
}

export function FlowSegmented<T extends string>({
  label,
  value,
  options,
  disabled,
  onChange,
}: {
  label: string;
  value: T;
  options: Array<{value: T; label: string}>;
  disabled?: boolean;
  onChange: (value: T) => void;
}) {
  return (
    <div className="fs-field">
      <div className="fs-field-label">{label}</div>
      <div
        className="fs-segmented"
        role="radiogroup"
        aria-label={label}
        data-empty={!options.some((opt) => opt.value === value)}
        style={{
          '--seg-count': options.length,
          '--seg-index': options.findIndex((opt) => opt.value === value),
        } as CSSProperties}
      >
        <span className="fs-segmented-indicator" aria-hidden="true" />
        {options.map((opt) => (
          <button
            key={opt.value}
            type="button"
            role="radio"
            aria-checked={value === opt.value}
            data-text={opt.label}
            disabled={disabled}
            className="fs-segmented-item"
            onClick={() => onChange(opt.value)}
          >
            {opt.label}
          </button>
        ))}
      </div>
    </div>
  );
}

export function FlowSelect({
  label,
  description,
  value,
  options,
  disabled,
  disabledMessage,
  onChange,
}: {
  label: string;
  description?: string;
  value: string;
  options: Array<{value: string; label: string; description?: string}>;
  disabled?: boolean;
  disabledMessage?: string;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [closing, setClosing] = useState(false);
  const [highlightedIdx, setHighlightedIdx] = useState(() =>
    Math.max(0, options.findIndex((o) => o.value === value)),
  );
  const [rect, setRect] = useState<{top: number; left: number; width: number} | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

  const selectedOpt = options.find((o) => o.value === value) ?? options[0];

  const closeMenu = () => {
    if (!open || closing) return;
    setClosing(true);
    window.setTimeout(() => {
      setOpen(false);
      setClosing(false);
    }, 140);
  };

  const toggleMenu = () => {
    if (disabled) return;
    if (open) {
      closeMenu();
      return;
    }
    const r = triggerRef.current?.getBoundingClientRect();
    if (r) {
      const top = Math.min(window.innerHeight - 290, r.bottom + 6);
      setRect({top: Math.max(8, top), left: r.left, width: Math.max(240, r.width)});
    }
    setHighlightedIdx(Math.max(0, options.findIndex((o) => o.value === value)));
    setOpen(true);
  };

  useEffect(() => {
    if (!open) return;
    const onOutside = (e: MouseEvent) => {
      const target = e.target as Node;
      if (triggerRef.current?.contains(target) || popoverRef.current?.contains(target)) return;
      closeMenu();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        closeMenu();
        triggerRef.current?.focus();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        setHighlightedIdx((i) => Math.min(options.length - 1, i + 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setHighlightedIdx((i) => Math.max(0, i - 1));
      } else if (e.key === 'Home') {
        e.preventDefault();
        setHighlightedIdx(0);
      } else if (e.key === 'End') {
        e.preventDefault();
        setHighlightedIdx(Math.max(0, options.length - 1));
      } else if (e.key === 'Enter' || e.key === ' ') {
        const picked = options[highlightedIdx];
        if (picked) {
          e.preventDefault();
          onChange(picked.value);
          closeMenu();
          triggerRef.current?.focus();
        }
      }
    };
    const onScroll = (e: Event) => {
      if (popoverRef.current?.contains(e.target as Node)) return;
      closeMenu();
    };
    document.addEventListener('mousedown', onOutside);
    window.addEventListener('keydown', onKey);
    window.addEventListener('scroll', onScroll, true);
    return () => {
      document.removeEventListener('mousedown', onOutside);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('scroll', onScroll, true);
    };
  });

  return (
    <div className="fs-field" title={disabled ? disabledMessage : undefined}>
      <span className="fs-field-label">{label}</span>
      <button
        ref={triggerRef}
        type="button"
        className="fs-select-trigger"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={label}
        onClick={toggleMenu}
        onKeyDown={(e) => {
          if (!open && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
            e.preventDefault();
            toggleMenu();
          }
        }}
      >
        <span className="fs-select-value">{selectedOpt?.label ?? value}</span>
        <ChevronDownIcon
          width={15}
          height={15}
          style={{
            flexShrink: 0,
            color: 'var(--fs-text-secondary)',
            transform: open && !closing ? 'rotate(180deg)' : 'rotate(0deg)',
            transition: 'transform 160ms var(--fs-ease-out)',
          }}
        />
      </button>
      {description && <span className="fs-field-desc">{description}</span>}

      {open &&
        rect &&
        createPortal(
          <div
            ref={popoverRef}
            className="fs-select-popover"
            role="listbox"
            aria-label={label}
            data-closing={closing ? 'true' : 'false'}
            style={{top: rect.top, left: rect.left, width: rect.width}}
          >
            {options.map((opt, idx) => (
              <button
                key={opt.value}
                type="button"
                role="option"
                aria-selected={opt.value === value}
                className="fs-select-option"
                style={
                  idx === highlightedIdx && opt.value !== value
                    ? {backgroundColor: 'var(--fs-bg-pill-hover)'}
                    : undefined
                }
                onMouseEnter={() => setHighlightedIdx(idx)}
                onClick={() => {
                  onChange(opt.value);
                  closeMenu();
                }}
              >
                <div style={{display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8}}>
                  <span className="fs-select-opt-label">{opt.label}</span>
                  {opt.value === value && <CheckIcon width={14} height={14} style={{color: 'var(--fs-accent)'}} />}
                </div>
                {opt.description && <span className="fs-select-opt-desc">{opt.description}</span>}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}

export function FlowSlider({
  label,
  description,
  value,
  min,
  max,
  step = 1,
  disabled,
  disabledMessage,
  formatValue,
  onChange,
}: {
  label: string;
  description?: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  disabledMessage?: string;
  formatValue?: (v: number) => string;
  onChange: (v: number) => void;
}) {
  const pct = max > min ? Math.min(100, Math.max(0, ((value - min) / (max - min)) * 100)) : 0;

  return (
    <div className="fs-field" title={disabled ? disabledMessage : undefined}>
      <div className="fs-field-label">{label}</div>
      <div className="fs-slider-row">
        <div className="fs-slider-track-wrap">
          <input
            type="range"
            className="fs-slider-range"
            value={value}
            min={min}
            max={max}
            step={step}
            disabled={disabled}
            aria-label={label}
            style={{
              background: `linear-gradient(to right, var(--fs-accent) ${pct}%, var(--fs-border-strong) ${pct}%)`,
            }}
            onChange={(e) => onChange(Number(e.target.value))}
          />
        </div>
        <span className="fs-slider-value">{formatValue ? formatValue(value) : value}</span>
      </div>
      {description && <div className="fs-field-desc">{description}</div>}
    </div>
  );
}

export function FlowNumber({
  label,
  description,
  value,
  min,
  max,
  step = 1,
  units,
  disabled,
  disabledMessage,
  onChange,
}: {
  label: string;
  description?: string;
  value: number;
  min?: number;
  max?: number;
  step?: number;
  units?: string;
  disabled?: boolean;
  disabledMessage?: string;
  onChange: (value: number) => void;
}) {
  const clamp = (n: number) => {
    let next = n;
    if (min !== undefined) next = Math.max(min, next);
    if (max !== undefined) next = Math.min(max, next);
    return Number(next.toFixed(3));
  };

  return (
    <div className="fs-field" title={disabled ? disabledMessage : undefined}>
      <span className="fs-field-label">{label}</span>
      <div className="fs-number-box">
        <input
          type="number"
          className="fs-number-input"
          value={value}
          min={min}
          max={max}
          step={step}
          disabled={disabled}
          aria-label={label}
          onChange={(e) => {
            const parsed = Number(e.target.value);
            if (!Number.isNaN(parsed)) onChange(parsed);
          }}
        />
        {units && <span className="fs-number-unit">{units}</span>}
        <button
          type="button"
          className="fs-number-step"
          disabled={disabled || (min !== undefined && value <= min)}
          aria-label={`Decrease ${label}`}
          onClick={() => onChange(clamp(value - step))}
        >
          −
        </button>
        <button
          type="button"
          className="fs-number-step"
          disabled={disabled || (max !== undefined && value >= max)}
          aria-label={`Increase ${label}`}
          onClick={() => onChange(clamp(value + step))}
        >
          +
        </button>
      </div>
      {description && <span className="fs-field-desc">{description}</span>}
    </div>
  );
}

export function FlowModal({
  isOpen,
  title,
  subtitle,
  onClose,
  children,
  footer,
  wide = false,
}: {
  isOpen: boolean;
  title: string;
  subtitle?: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
}) {
  const [rendered, setRendered] = useState(isOpen);
  const [closing, setClosing] = useState(false);
  const [origin, setOrigin] = useState<string>('50% 50%');

  useEffect(() => {
    if (isOpen) {
      const el = document.activeElement;
      if (el instanceof HTMLElement) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) {
          const ox = Math.round(((r.left + r.width / 2) / window.innerWidth) * 100);
          const oy = Math.round(((r.top + r.height / 2) / window.innerHeight) * 100);
          setOrigin(`${ox}% ${oy}%`);
        }
      }
      setRendered(true);
      setClosing(false);
    } else if (rendered) {
      setClosing(true);
      const t = window.setTimeout(() => {
        setRendered(false);
        setClosing(false);
      }, 160);
      return () => window.clearTimeout(t);
    }
  }, [isOpen, rendered]);

  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [isOpen, onClose]);

  if (!rendered) return null;
  return createPortal(
    <div
      className="fs-modal-backdrop"
      data-closing={closing ? 'true' : 'false'}
      onMouseDown={onClose}
    >
      <div
        className="fs-modal"
        data-wide={wide || undefined}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        style={{transformOrigin: origin}}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="fs-modal-header">
          <div>
            <div className="fs-card-title">{title}</div>
            {subtitle && <div className="fs-card-subtitle">{subtitle}</div>}
          </div>
          <FlowButton
            variant="ghost"
            ariaLabel="Close dialog"
            icon={<XMarkIcon width={16} height={16} />}
            onClick={onClose}
          />
        </div>
        <div className="fs-modal-body">{children}</div>
        {footer && <div className="fs-modal-footer">{footer}</div>}
      </div>
    </div>,
    document.body,
  );
}

export function FlowCrossfade({
  activeKey,
  direction = 'none',
  className = '',
  children,
}: {
  activeKey: string;
  direction?: 'none' | 'left' | 'right';
  className?: string;
  children: ReactNode;
}) {
  const [exiting, setExiting] = useState<{key: string; node: ReactNode} | null>(null);
  const settledRef = useRef<{key: string; node: ReactNode}>({key: activeKey, node: children});

  useEffect(() => {
    if (settledRef.current.key !== activeKey) {
      setExiting(settledRef.current);
      settledRef.current = {key: activeKey, node: children};
      const t = window.setTimeout(() => setExiting(null), 160);
      return () => window.clearTimeout(t);
    }
    settledRef.current = {key: activeKey, node: children};
  }, [activeKey, children]);

  const enterClass =
    direction === 'right'
      ? 'fs-view-enter-right'
      : direction === 'left'
        ? 'fs-view-enter-left'
        : 'fs-view-enter';

  return (
    <div className={`fs-crossfade-stage ${className}`.trim()}>
      {exiting && (
        <div key={`exit-${exiting.key}`} className="fs-crossfade-exit" aria-hidden="true">
          {exiting.node}
        </div>
      )}
      <div key={activeKey} className={enterClass}>
        {children}
      </div>
    </div>
  );
}
