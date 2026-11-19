// The flowscale theme. `astryx experimental theme editor` wrote this file once and never changes it again.
// The checked settings live in flowscale.settings.json and the generated flowscale.settings.ts;
// change them with `astryx experimental theme editor set flowscale ...`. Add your own changes below: they win over the settings.
import {defineTheme} from '@astryxdesign/core/theme';
import {flowscaleSettings} from './flowscale.settings';

export const flowscaleTheme = defineTheme({
  extends: flowscaleSettings,
  name: 'flowscale',
  typography: {
    scale: {base: 15, ratio: 1.16},
    body: {
      family: 'system-ui',
      fallbacks: '"Segoe UI Variable Text", "Segoe UI", -apple-system, sans-serif',
    },
    heading: {
      family: 'system-ui',
      fallbacks: '"Segoe UI Variable Display", "Segoe UI", -apple-system, sans-serif',
      weight: 'semibold',
      weights: {1: 'semibold', 2: 'semibold', 3: 'semibold', 4: 'semibold'},
    },
    code: {
      family: 'ui-monospace',
      fallbacks: '"Cascadia Mono", Consolas, monospace',
    },
  },
  motion: {tempo: 0.85, character: 'crisp'},
  tokens: {
    '--color-background-body': ['#F5F6F7', '#0F0F10'],
    '--color-background-surface': ['#FFFFFF', '#141415'],
    '--color-background-card': ['#FFFFFF', '#19191B'],
    '--color-background-popover': ['#FFFFFF', '#1D1D20'],
    '--color-background-muted': ['#EDEEF0', '#1D1D20'],
    '--color-border': ['#E1E3E6', '#26262A'],
    '--color-border-emphasized': ['#C6CAD0', '#3A3A40'],
    '--color-text-primary': ['#141517', '#EDEDED'],
    '--color-text-secondary': ['#52565E', '#A1A1A6'],
    '--color-text-disabled': ['#8A8F99', '#6E6E74'],
    '--color-icon-primary': ['#141517', '#EDEDED'],
    '--color-icon-secondary': ['#52565E', '#A1A1A6'],
    '--color-icon-disabled': ['#8A8F99', '#6E6E74'],
    '--shadow-low': '0 1px 2px light-dark(#00000014, #00000059)',
  },
  components: {
    button: {
      base: {
        fontWeight: 'var(--font-weight-medium)',
      },
    },
  },
});
