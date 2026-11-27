/**
 * KT-1887 — the ESLint rules that keep the UI on the design system.
 *
 * Each rule refuses new code that drifts from it; the violations that existed
 * when a rule came in are its baseline, recorded in `eslint-suppressions.json`
 * (ESLint bulk suppressions), so `pnpm lint` fails on one beyond the baseline
 * and on a baseline entry no violation matches any more (`pnpm lint
 * --prune-suppressions` removes it). The stylesheet half of the off-scale
 * check (font sizes, radii, colours and breakpoints in `.css`) is in
 * `src/styles/themeAudit.ts`, run by `pnpm lint:theme`.
 *
 * Rules (plugin `ui-guards`):
 *   - no-literal-ui-string       a literal UI string in JSX (text, aria-label, title…)
 *   - control-has-name           an icon-only <button>, or an <input>/<select>/<textarea>, without a name
 *   - no-adhoc-dialog            role="dialog"/"alertdialog", aria-modal or <dialog> outside components/ui
 *   - no-foreign-page-stylesheet a page or a component importing the stylesheet of another page
 *   - no-off-scale-style         an inline font size, radius or colour outside the tokens, a breakpoint outside BREAKPOINTS
 *
 * See docs/conventions/ui-lint-guards.md.
 */
import { basename, dirname, extname, resolve } from 'node:path';
import {
  BREAKPOINT_PX, dropFallbacks, FONT_SIZE_OK, hasLiteralColour, radiusOk,
} from '../src/styles/themeAudit.ts';

const DOC = 'See docs/conventions/ui-lint-guards.md.';

// ── Shared helpers ─────────────────────────────────────────────────────────

const attrName = attr => attr.type === 'JSXAttribute'
  ? (attr.name.type === 'JSXNamespacedName' ? `${attr.name.namespace.name}:${attr.name.name.name}` : attr.name.name)
  : null;

const elementName = node => {
  const n = node.name;
  if (n.type === 'JSXIdentifier') return n.name;
  if (n.type === 'JSXMemberExpression') return `${elementName({ name: n.object })}.${n.property.name}`;
  return null;
};

const findAttr = (opening, name) => opening.attributes.find(a => attrName(a) === name);
const hasSpread = opening => opening.attributes.some(a => a.type === 'JSXSpreadAttribute');

/** The static string an attribute holds, or null when it is computed. */
function staticAttrValue(attr) {
  const v = attr?.value;
  if (!v) return null;
  if (v.type === 'Literal') return typeof v.value === 'string' ? v.value : null;
  if (v.type === 'JSXExpressionContainer') {
    const e = v.expression;
    if (e.type === 'Literal' && typeof e.value === 'string') return e.value;
    if (e.type === 'TemplateLiteral' && e.expressions.length === 0) return e.quasis[0].value.cooked;
  }
  return null;
}

/** An attribute that gives a name: present and not an empty literal. */
function names(attr) {
  if (!attr) return false;
  if (!attr.value) return false;
  const s = staticAttrValue(attr);
  return s === null || s.trim() !== '';
}

const HAS_LETTER = /\p{L}/u;

// ── no-literal-ui-string ──────────────────────────────────────────────────

/** Attributes whose value a reader hears or sees. */
export const UI_TEXT_ATTRS = new Set([
  'aria-label', 'aria-description', 'aria-placeholder', 'aria-roledescription', 'aria-valuetext',
  'title', 'placeholder', 'alt', 'label',
]);

/** Elements whose text is code or a key, not a translatable sentence. */
const CODE_ELEMENTS = new Set(['code', 'kbd', 'pre', 'samp', 'var']);

/** A word that is not a sentence to translate: a path, URL, file name, key or
 *  command (no space, holds `/ . _ $ ~ = @ { }`), an acronym or symbol (no
 *  lower-case letter, at most six characters: GET, PDF, MCP, Σ), or a unit. */
const CODE_WORD = /^[^\s]*[/._$~=@{}\\][^\s]*$/;
const ACRONYM = /^[^\p{Ll}]{1,6}$/u;
const UNIT = /^(?:s|ms|min|h|px|tok|tk|k|kb|mb|gb|v|x)$/i;
const PUNCTUATION = /^[\p{P}\p{S}\d\s]*$/u;

/** A command-line flag or a kebab-case identifier (`--output`, `my-project`). */
const KEBAB = /^(?:-{1,2}[a-z][a-z0-9-]*|[a-z0-9]+(?:-[a-z0-9]+)+)$/;

/** Product names, written the same in every locale. */
export const PRODUCT_NAMES = new Set([
  'Kronn', 'Claude', 'Codex', 'Gemini', 'Copilot', 'Kiro', 'Vibe', 'Ollama', 'ComfyUI', 'Tailscale',
  'GitHub', 'GitLab', 'macOS', 'Linux', 'Windows', 'ccusage',
]);

export function isUntranslatable(text) {
  const words = text.trim().split(/\s+/).filter(w => !PUNCTUATION.test(w));
  if (words.length === 0) return true;
  return words.every(w => {
    const bare = w.replace(/^[\p{P}\p{S}]+|[\p{P}\p{S}]+$/gu, '');
    return CODE_WORD.test(w) || KEBAB.test(w) || ACRONYM.test(w) || UNIT.test(bare) || PRODUCT_NAMES.has(bare);
  });
}

function insideCode(node) {
  for (let p = node.parent; p; p = p.parent) {
    if (p.type === 'JSXElement' && CODE_ELEMENTS.has(elementName(p.openingElement))) return true;
  }
  return false;
}

const noLiteralUiString = {
  meta: {
    type: 'suggestion',
    docs: { description: 'A UI string in JSX comes from t(), not a literal.' },
    schema: [],
    messages: {
      text: `Literal UI text "{{text}}": write it in the four locales and render t('…'). ${DOC}`,
      attr: `Literal {{attr}}="{{text}}": write it in the four locales and pass t('…'). ${DOC}`,
    },
  },
  create(context) {
    return {
      JSXText(node) {
        const text = node.value.trim();
        if (!HAS_LETTER.test(text) || isUntranslatable(text) || insideCode(node)) return;
        context.report({ node, messageId: 'text', data: { text: text.slice(0, 40) } });
      },
      JSXAttribute(node) {
        const name = attrName(node);
        if (!UI_TEXT_ATTRS.has(name)) return;
        const text = staticAttrValue(node);
        if (text === null || !HAS_LETTER.test(text) || isUntranslatable(text)) return;
        context.report({ node, messageId: 'attr', data: { attr: name, text: text.trim().slice(0, 40) } });
      },
    };
  },
};

// ── control-has-name ──────────────────────────────────────────────────────

const UNNAMED_INPUT_TYPES = new Set(['hidden', 'submit', 'reset', 'button', 'image']);

/** True when a JSX child can carry text a reader hears. Conservative: an
 *  expression or an unknown component counts as text; an icon does not. */
function carriesText(child, icons) {
  if (child.type === 'JSXText') return child.value.trim() !== '';
  if (child.type === 'JSXExpressionContainer') return child.expression.type !== 'JSXEmptyExpression';
  if (child.type === 'JSXFragment') return child.children.some(c => carriesText(c, icons));
  if (child.type !== 'JSXElement') return false;
  const opening = child.openingElement;
  const name = elementName(opening);
  const hidden = findAttr(opening, 'aria-hidden');
  if (hidden && (hidden.value === null || staticAttrValue(hidden) === 'true'
    || (hidden.value?.type === 'JSXExpressionContainer' && hidden.value.expression.value === true))) return false;
  if (name === 'img') return names(findAttr(opening, 'alt'));
  if (name === 'svg') return Boolean(findAttr(opening, 'aria-label')) || child.children.some(c => c.type === 'JSXElement' && elementName(c.openingElement) === 'title');
  if (icons.has(name) || /Icon$/.test(name ?? '')) return false;
  if (child.children.length === 0) return /^[a-z]/.test(name ?? '') ? false : true;
  return child.children.some(c => carriesText(c, icons));
}

/** Inside a <label>, or the direct child of Field (which gives it an id bound
 *  to its label). */
function insideLabel(node) {
  const parent = node.parent;
  if (parent?.type === 'JSXElement' && elementName(parent.openingElement) === 'Field') return true;
  for (let p = node.parent; p; p = p.parent) {
    if (p.type === 'JSXElement' && elementName(p.openingElement) === 'label') return true;
  }
  return false;
}

/** `hidden`, `aria-hidden="true"` or `style={{ display: 'none' }}`: out of
 *  the accessibility tree. */
function outOfTree(opening) {
  if (findAttr(opening, 'hidden') || staticAttrValue(findAttr(opening, 'aria-hidden')) === 'true') return true;
  const style = findAttr(opening, 'style')?.value;
  if (style?.type !== 'JSXExpressionContainer' || style.expression.type !== 'ObjectExpression') return false;
  return style.expression.properties.some(p => p.type === 'Property' && p.key.type === 'Identifier'
    && p.key.name === 'display' && p.value.type === 'Literal' && p.value.value === 'none');
}

const controlHasName = {
  meta: {
    type: 'problem',
    docs: { description: 'An icon-only button and a form field have an accessible name.' },
    schema: [],
    messages: {
      button: `This icon-only <button> has no name: use IconButton (label=…), or give it aria-label. ${DOC}`,
      field: `This <{{tag}}> has no name: wrap it in Field (label=…), or give it a <label htmlFor>, aria-label, aria-labelledby or title (a placeholder is not a name). ${DOC}`,
    },
  },
  create(context) {
    const icons = new Set();
    const htmlFor = new Set();
    const pending = [];
    const valueText = attr => {
      const s = staticAttrValue(attr);
      if (s !== null) return `"${s}"`;
      return attr?.value?.type === 'JSXExpressionContainer' ? context.sourceCode.getText(attr.value.expression) : null;
    };
    return {
      ImportDeclaration(node) {
        if (node.source.value !== 'lucide-react') return;
        for (const s of node.specifiers) icons.add(s.local.name);
      },
      JSXAttribute(node) {
        if (attrName(node) === 'htmlFor') {
          const v = valueText(node);
          if (v) htmlFor.add(v);
        }
      },
      JSXElement(node) {
        const opening = node.openingElement;
        const tag = elementName(opening);
        if (tag !== 'button' && tag !== 'input' && tag !== 'select' && tag !== 'textarea') return;
        if (hasSpread(opening) || outOfTree(opening)) return;
        if (['aria-label', 'aria-labelledby', 'title'].some(a => names(findAttr(opening, a)))) return;
        if (tag === 'button') {
          if (!node.children.some(c => carriesText(c, icons))) context.report({ node: opening, messageId: 'button' });
          return;
        }
        // A placeholder does not name a field here, although HTML-AAM and
        // axe fall back on it: it disappears as soon as the user types, so it
        // is not a label (WCAG 3.3.2).
        if (tag === 'input') {
          const type = staticAttrValue(findAttr(opening, 'type'));
          if (type && UNNAMED_INPUT_TYPES.has(type)) return;
        }
        if (insideLabel(node)) return;
        pending.push({ opening, tag, id: valueText(findAttr(opening, 'id')) });
      },
      'Program:exit'() {
        for (const { opening, tag, id } of pending) {
          if (id && htmlFor.has(id)) continue;
          context.report({ node: opening, messageId: 'field', data: { tag } });
        }
      },
    };
  },
};

// ── no-adhoc-dialog ───────────────────────────────────────────────────────

const noAdhocDialog = {
  meta: {
    type: 'problem',
    docs: { description: 'A dialog is built with Dialog, Drawer or ConfirmDialog (components/ui).' },
    schema: [],
    messages: {
      dialog: `{{what}} outside components/ui: build the dialog with Dialog, Drawer or ConfirmDialog (focus trap, Escape, focus return, name). ${DOC}`,
    },
  },
  create(context) {
    return {
      // One report per element. Dialog is modal: a non-modal popover or an
      // inline confirmation with role="dialog" and no aria-modal is another
      // pattern, left alone.
      JSXOpeningElement(node) {
        if (elementName(node) === 'dialog') {
          context.report({ node, messageId: 'dialog', data: { what: '<dialog>' } });
          return;
        }
        const modal = findAttr(node, 'aria-modal');
        if (!modal) return;
        const value = staticAttrValue(modal);
        const literalFalse = value === 'false'
          || (modal.value?.type === 'JSXExpressionContainer' && modal.value.expression.type === 'Literal' && modal.value.expression.value === false);
        if (literalFalse) return;
        const role = staticAttrValue(findAttr(node, 'role'));
        context.report({ node, messageId: 'dialog', data: { what: role ? `role="${role}" aria-modal` : 'aria-modal' } });
      },
    };
  },
};

// ── no-foreign-page-stylesheet ────────────────────────────────────────────

const noForeignPageStylesheet = {
  meta: {
    type: 'problem',
    docs: { description: 'Only a page imports its own stylesheet (src/pages/<Page>.css).' },
    schema: [],
    messages: {
      foreign: `{{css}} is the stylesheet of the {{page}} page: a change made for that page would break this file. Move the shared classes into a components/ui component or a stylesheet of their own. ${DOC}`,
    },
  },
  create(context) {
    const file = context.filename;
    return {
      ImportDeclaration(node) {
        const source = node.source.value;
        if (typeof source !== 'string' || !source.endsWith('.css') || !source.startsWith('.')) return;
        const target = resolve(dirname(file), source);
        // A page stylesheet sits directly in src/pages.
        if (basename(dirname(target)) !== 'pages' || basename(dirname(dirname(target))) !== 'src') return;
        const page = basename(target, '.css');
        const own = dirname(file) === dirname(target) && basename(file, extname(file)) === page;
        if (!own) context.report({ node, messageId: 'foreign', data: { css: `pages/${page}.css`, page } });
      },
    };
  },
};

// ── no-off-scale-style ────────────────────────────────────────────────────

// The scales are defined once, in themeAudit.ts, for this rule and the
// stylesheet check alike: BREAKPOINT_PX, FONT_SIZE_OK, radiusOk, hasLiteralColour.
const BREAKPOINTS = new Set(BREAKPOINT_PX);

const RADIUS_PROPS = /^border(?:(?:Top|Bottom|Start|End)(?:Left|Right|Start|End))?Radius$/;
const COLOUR_PROPS = new Set([
  'color', 'background', 'backgroundColor', 'borderColor', 'borderTopColor', 'borderRightColor',
  'borderBottomColor', 'borderLeftColor', 'border', 'borderTop', 'borderRight', 'borderBottom', 'borderLeft',
  'outline', 'outlineColor', 'fill', 'stroke', 'boxShadow', 'textShadow', 'caretColor', 'accentColor',
  'textDecorationColor',
]);

const keyName = prop => prop.key.type === 'Identifier' ? prop.key.name
  : prop.key.type === 'Literal' && typeof prop.key.value === 'string' ? prop.key.value : null;

/** The static text of a style value: a literal or a template without
 *  expressions; a template with expressions gives its literal parts. */
function styleValue(node) {
  if (node.type === 'Literal') return typeof node.value === 'number' ? node.value : typeof node.value === 'string' ? node.value : null;
  if (node.type === 'TemplateLiteral') return node.quasis.map(q => q.value.cooked).join('${}');
  return null;
}

const noOffScaleStyle = {
  meta: {
    type: 'suggestion',
    docs: { description: 'Inline font sizes, radii and colours come from the tokens; breakpoints from BREAKPOINTS.' },
    schema: [],
    messages: {
      fontSize: `fontSize {{value}} is off the type scale: use var(--kr-type-*) (or a class). ${DOC}`,
      radius: `{{prop}} {{value}} is off the radius scale: use var(--kr-radius-control|surface|overlay|pill). ${DOC}`,
      colour: `{{prop}} {{value}} is a literal colour: read a --kr-* token. ${DOC}`,
      breakpoint: `{{value}} px is not a breakpoint: use BREAKPOINTS.sm|md|lg (640, 768, 900) from styles/designTokens. ${DOC}`,
    },
  },
  create(context) {
    const checkStyle = obj => {
      for (const prop of obj.properties) {
        if (prop.type !== 'Property' || prop.computed) continue;
        const key = keyName(prop);
        if (!key) continue;
        const value = styleValue(prop.value);
        if (value === null) continue;
        const text = String(value).trim();
        // A template with expressions computes its size: only its literal
        // parts are read, for a colour.
        const computed = prop.value.type === 'TemplateLiteral' && prop.value.expressions.length > 0;
        if (key === 'fontSize') {
          if (!computed && (typeof value === 'number' || !FONT_SIZE_OK.test(dropFallbacks(text)))) {
            context.report({ node: prop, messageId: 'fontSize', data: { value: text } });
          }
        } else if (RADIUS_PROPS.test(key)) {
          if (!computed && ((typeof value === 'number' && value !== 0) || (typeof value === 'string' && !radiusOk(text)))) {
            context.report({ node: prop, messageId: 'radius', data: { prop: key, value: text } });
          }
        } else if (COLOUR_PROPS.has(key) && typeof value === 'string' && hasLiteralColour(text)) {
          context.report({ node: prop, messageId: 'colour', data: { prop: key, value: text.slice(0, 40) } });
        }
      }
    };
    const checkWidth = (node, px) => {
      if (!BREAKPOINTS.has(px)) context.report({ node, messageId: 'breakpoint', data: { value: String(px) } });
    };
    return {
      JSXAttribute(node) {
        if (attrName(node) !== 'style' || node.value?.type !== 'JSXExpressionContainer') return;
        if (node.value.expression.type === 'ObjectExpression') checkStyle(node.value.expression);
      },
      CallExpression(node) {
        const callee = node.callee.type === 'Identifier' ? node.callee.name
          : node.callee.type === 'MemberExpression' && node.callee.property.type === 'Identifier' ? node.callee.property.name : null;
        const [arg] = node.arguments;
        if (!arg) return;
        if (callee === 'useIsMobile' && arg.type === 'Literal' && typeof arg.value === 'number') checkWidth(arg, arg.value);
        if (callee === 'matchMedia' || callee === 'useMediaQuery') {
          const text = styleValue(arg);
          if (typeof text !== 'string') return;
          for (const m of text.matchAll(/\((max|min)-width\s*:\s*(\d+(?:\.\d+)?)px\)/g)) {
            const px = Number(m[2]);
            checkWidth(arg, m[1] === 'max' ? px + 1 : px);
          }
        }
      },
    };
  },
};

export default {
  meta: { name: 'ui-guards', version: '1.0.0' },
  rules: {
    'no-literal-ui-string': noLiteralUiString,
    'control-has-name': controlHasName,
    'no-adhoc-dialog': noAdhocDialog,
    'no-foreign-page-stylesheet': noForeignPageStylesheet,
    'no-off-scale-style': noOffScaleStyle,
  },
};
