import { useEffect, useRef, useState } from 'react';
import { motion, useReducedMotion } from 'motion/react';
import { listImages } from '../lib/api';
import type { ImageMetadata } from '../lib/types';
import { ImageGallery } from './ImageGallery';
import { ImageGeneratorPanel } from './ImageGeneratorPanel';

export function ImagesView() {
  const reduceMotion = useReducedMotion();
  const [images, setImages] = useState<ImageMetadata[]>([]);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const generatedIdsRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    let active = true;
    setBusy(true);
    void listImages(60, 0)
      .then((response) => {
        if (!active) return;
        setImages((current) => {
          const generated = current.filter((image) =>
            generatedIdsRef.current.has(image.image_id),
          );
          const fetched = new Map(response.images.map((image) => [image.image_id, image]));
          for (const image of generated) fetched.delete(image.image_id);
          return [...generated, ...fetched.values()];
        });
        setError(null);
      })
      .catch((reason: unknown) => {
        if (!active) return;
        setError(reason instanceof Error ? reason.message : 'Unknown error');
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
    };
  }, []);

  const handleGenerated = (image: ImageMetadata) => {
    generatedIdsRef.current.add(image.image_id);
    setImages((current) => [
      image,
      ...current.filter((existing) => existing.image_id !== image.image_id),
    ]);
    setError(null);
  };

  return (
    <motion.section
      aria-labelledby="images-heading"
      className="flex flex-col gap-5"
      initial={{ opacity: 0, y: reduceMotion ? 0 : 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduceMotion ? 0 : 0.28, ease: 'easeOut' }}
    >
      <header>
        <h2 id="images-heading" className="text-2xl font-bold text-slate-100">
          Images
        </h2>
        <p className="mt-1 text-sm text-slate-300">
          Generate an image with a configured provider and browse saved results.
        </p>
      </header>

      <div className="grid gap-6 lg:grid-cols-[minmax(18rem,22rem)_minmax(0,1fr)]">
        <div className="image-card h-fit min-w-0">
          <ImageGeneratorPanel onGenerated={handleGenerated} />
        </div>
        <ImageGallery images={images} loading={busy} error={error} />
      </div>
    </motion.section>
  );
}
