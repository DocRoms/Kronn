import '../pages/DiscussionsPage.css';

interface CollectionSidebarFooterProps {
  label: string;
  navigateLabel: string;
  searchLabel: string;
}

/** Shared keyboard-hint footer for CollectionShell sidebars. */
export function CollectionSidebarFooter({
  label,
  navigateLabel,
  searchLabel,
}: CollectionSidebarFooterProps) {
  return (
    <div className="disc-sidebar-footer">
      <span>{label}</span>
      <span>
        <kbd>↑↓</kbd> {navigateLabel}
        <span aria-hidden="true"> · </span>
        <kbd>/</kbd> {searchLabel}
      </span>
    </div>
  );
}
