import { imageUrl } from '../lib/api';
import type { ImageMetadata } from '../lib/types';

interface ImageGalleryProps {
  images: ImageMetadata[];
  loading: boolean;
  error: string | null;
}

// Fixed ids keep skeleton cards stable across renders; array indexes are never keys.
const SKELETON_IDS = [
  'image-skeleton-1',
  'image-skeleton-2',
  'image-skeleton-3',
  'image-skeleton-4',
  'image-skeleton-5',
  'image-skeleton-6',
];

function localTimestamp(epochMs: number): string {
  return new Date(epochMs).toLocaleString();
}

export function ImageGallery({ images, loading, error }: ImageGalleryProps) {
  return (
    <section aria-labelledby="image-gallery-heading" className="min-w-0">
      <h2 id="image-gallery-heading" className="mb-3 text-lg font-semibold text-slate-100">
        Gallery
      </h2>

      {error ? (
        <p role="alert" className="image-error-summary text-sm">
          {`Could not load images: ${error}`}
        </p>
      ) : loading ? (
        <ul className="image-grid" aria-busy="true">
          {SKELETON_IDS.map((id) => (
            <li
              key={id}
              data-testid="image-skeleton"
              aria-busy="true"
              className="image-skeleton shimmer-sweep"
            />
          ))}
        </ul>
      ) : images.length === 0 ? (
        <p className="rounded-lg border border-[var(--image-border)] bg-[var(--image-card)] p-4 text-sm text-slate-200">
          No generated images yet
        </p>
      ) : (
        <ul className="image-grid">
          {images.map((image) => (
            <li key={image.image_id} className="image-card">
              <img
                src={imageUrl(image.image_id)}
                alt={image.prompt}
                loading="lazy"
                width={512}
                height={512}
              />
              <p className="mt-2 text-sm text-slate-200">
                Provider: {image.provider} · Model: {image.model} · Size: {image.size}
              </p>
              <p className="mt-1 text-xs text-slate-300">{localTimestamp(image.created_at_epoch_ms)}</p>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
