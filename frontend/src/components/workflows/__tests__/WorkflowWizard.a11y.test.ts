import fs from 'node:fs';
import path from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const CONTROL_TAGS = new Set(['input', 'select', 'textarea']);

describe('WorkflowWizard form accessibility', () => {
  it('gives every native form control an accessible name', () => {
    const sourcePath = path.resolve(process.cwd(), 'src/components/workflows/WorkflowWizard.tsx');
    const source = fs.readFileSync(sourcePath, 'utf8');
    const file = ts.createSourceFile(sourcePath, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const labelledIds = new Set<string>();

    const collectLabels = (node: ts.Node) => {
      if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
        if (node.tagName.getText(file) === 'label') {
          const htmlFor = node.attributes.properties.find(
            (property): property is ts.JsxAttribute => (
              ts.isJsxAttribute(property) && property.name.getText(file) === 'htmlFor'
            ),
          );
          if (htmlFor?.initializer && ts.isStringLiteral(htmlFor.initializer)) {
            labelledIds.add(htmlFor.initializer.text);
          }
        }
      }
      ts.forEachChild(node, collectLabels);
    };
    collectLabels(file);

    const unnamed: string[] = [];
    const visit = (node: ts.Node, insideLabel = false) => {
      const nextInsideLabel = insideLabel || (
        ts.isJsxElement(node) && node.openingElement.tagName.getText(file) === 'label'
      );
      if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
        const tag = node.tagName.getText(file);
        if (CONTROL_TAGS.has(tag)) {
          const attributes = node.attributes.properties.filter(ts.isJsxAttribute);
          const names = new Set(attributes.map(attribute => attribute.name.getText(file)));
          const idAttribute = attributes.find(attribute => attribute.name.getText(file) === 'id');
          const id = idAttribute?.initializer && ts.isStringLiteral(idAttribute.initializer)
            ? idAttribute.initializer.text
            : null;
          if (
            !nextInsideLabel &&
            !names.has('aria-label') &&
            !names.has('aria-labelledby') &&
            !(id && labelledIds.has(id))
          ) {
            const { line } = file.getLineAndCharacterOfPosition(node.getStart(file));
            unnamed.push(`${tag} at line ${line + 1}`);
          }
        }
      }
      ts.forEachChild(node, child => visit(child, nextInsideLabel));
    };
    visit(file);

    expect(unnamed).toEqual([]);
  });
});
