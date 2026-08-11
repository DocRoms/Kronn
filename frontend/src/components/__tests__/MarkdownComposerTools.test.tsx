import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { MarkdownEditor } from '../MarkdownComposerTools';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

describe('MarkdownEditor', () => {
  it('renders the current Markdown in the same surface', () => {
    render(
      <MarkdownEditor content={'# Titre\n\n> Citation\n\n**fort**'}>
        <textarea aria-label="source" />
      </MarkdownEditor>,
    );
    fireEvent.click(screen.getByRole('tab', { name: 'markdown.preview' }));

    expect(screen.getByRole('heading', { name: 'Titre' })).toBeInTheDocument();
    expect(screen.getByText('Citation').closest('blockquote')).not.toBeNull();
    expect(screen.getByText('fort').tagName).toBe('STRONG');
    expect(screen.getByRole('tabpanel', { name: 'markdown.preview' })).toBeVisible();
    expect(screen.getByLabelText('source')).not.toBeVisible();
  });

  it('reads an uncontrolled textarea only when preview is requested', () => {
    render(
      <>
        <MarkdownEditor sourceId="source">
          <textarea id="source" defaultValue="- premier" />
        </MarkdownEditor>
      </>,
    );
    const source = document.getElementById('source') as HTMLTextAreaElement;
    source.value = '- nouveau';
    fireEvent.click(screen.getByRole('tab', { name: 'markdown.preview' }));

    expect(screen.getByText('nouveau').closest('li')).not.toBeNull();
    expect(screen.queryByText('premier')).toBeNull();
  });

  it('shows a concise syntax cheat sheet and closes it with Escape', () => {
    render(<MarkdownEditor content=""><textarea /></MarkdownEditor>);
    fireEvent.click(screen.getByRole('button', { name: 'markdown.help' }));
    expect(screen.getByText('**texte**')).toBeInTheDocument();
    expect(screen.getByText('[texte](url)')).toBeInTheDocument();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByText('**texte**')).toBeNull();
  });

  it('inserts a syntax example at the textarea caret and returns to edit mode', () => {
    function ControlledEditor() {
      const [value, setValue] = useState('Avant après');
      return (
        <MarkdownEditor content={value} sourceId="markdown-source">
          <textarea
            id="markdown-source"
            aria-label="source"
            value={value}
            onChange={event => setValue(event.target.value)}
          />
        </MarkdownEditor>
      );
    }
    render(<ControlledEditor />);
    const source = screen.getByLabelText('source') as HTMLTextAreaElement;
    source.setSelectionRange(6, 6);

    fireEvent.click(screen.getByRole('button', { name: 'markdown.help' }));
    fireEvent.click(screen.getByRole('button', { name: /markdown\.insertExample: \*\*texte\*\*/ }));

    expect(source).toHaveValue('Avant **texte**après');
    expect(screen.getByRole('tab', { name: 'markdown.edit' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.queryByRole('dialog', { name: 'markdown.help' })).toBeNull();
  });

  it('offers insertable emoji examples and a documentation link when enabled', () => {
    render(
      <MarkdownEditor sourceId="emoji-source" showEmojiHelp>
        <textarea id="emoji-source" aria-label="source" />
      </MarkdownEditor>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'markdown.help' }));

    expect(screen.getByText(':smile:')).toBeInTheDocument();
    const docsLink = screen.getByRole('link', { name: 'markdown.emojiDocs' });
    expect(docsLink).toHaveAttribute('href', expect.stringContaining('docs.github.com'));

    fireEvent.click(screen.getByText(':smile:').closest('button') as HTMLButtonElement);
    expect(screen.getByLabelText('source')).toHaveValue('😄');
  });

  it('uses the same close rules for every floating panel', () => {
    render(<MarkdownEditor content="# Titre"><textarea /></MarkdownEditor>);
    const edit = screen.getByRole('tab', { name: 'markdown.edit' });
    const preview = screen.getByRole('tab', { name: 'markdown.preview' });
    const help = screen.getByRole('button', { name: 'markdown.help' });

    fireEvent.click(preview);
    expect(preview).toHaveAttribute('aria-selected', 'true');
    fireEvent.click(edit);
    expect(edit).toHaveAttribute('aria-selected', 'true');

    fireEvent.click(help);
    expect(screen.getByRole('dialog', { name: 'markdown.help' })).toBeInTheDocument();
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole('dialog', { name: 'markdown.help' })).toBeNull();
  });
});
