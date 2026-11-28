import { describe, expect, it } from 'vitest';
import {
  buildOpenAIImageRequestBody,
  MockOpenAIImageProvider,
  parseOpenAIImageResponse,
} from './provider';

describe('cloud images', () => {
  it('builds an OpenAI-compatible request body without secrets', () => {
    const body = buildOpenAIImageRequestBody(
      { prompt: 'a red bicycle', size: '1024x1024' },
      'synthetic-image-model',
    );
    expect(body).toEqual({
      model: 'synthetic-image-model',
      prompt: 'a red bicycle',
      size: '1024x1024',
      response_format: 'b64_json',
    });
    expect(JSON.stringify(body)).not.toMatch(/sk-|Bearer/i);
  });

  it('parses synthetic responses and rejects malformed ones', () => {
    const parsed = parseOpenAIImageResponse(
      { id: 'image-1', data: [{ b64_json: 'c3ludGhldGlj' }] },
      'synthetic-image-model',
    );
    expect(parsed.mimeType).toBe('image/png');
    expect(() => parseOpenAIImageResponse({ data: [] }, 'm')).toThrow('data');
  });

  it('generates without a real credential', async () => {
    const image = await new MockOpenAIImageProvider().generate(
      { prompt: 'a red bicycle', size: '1024x1024' },
      'synthetic-key',
    );
    expect(image.id).toBe('synthetic-image');
  });
});
