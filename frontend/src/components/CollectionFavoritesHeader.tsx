import { ChevronRight, Star } from 'lucide-react';
import '../pages/DiscussionsPage.css';

interface CollectionFavoritesHeaderProps {
  label: string;
  count: number;
  expanded: boolean;
  onToggle: () => void;
}

/** Canonical Favorites section header shared by every collection sidebar. */
export function CollectionFavoritesHeader({
  label,
  count,
  expanded,
  onToggle,
}: CollectionFavoritesHeaderProps) {
  return (
    <button
      type="button"
      className="disc-group-btn collection-favorites-header"
      data-no-border="true"
      onClick={onToggle}
      aria-expanded={expanded}
    >
      <ChevronRight size={10} className="disc-chevron" data-expanded={expanded} />
      <Star size={10} className="collection-favorites-header-star" />
      <span className="collection-favorites-header-label">{label}</span>
      <span className="disc-group-count">{count}</span>
    </button>
  );
}
