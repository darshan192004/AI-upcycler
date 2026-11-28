export type ImageSize = '1024x1024' | '1024x1536' | '1536x1024';

export interface GenerateImageRequest {
  prompt: string;
  size: ImageSize;
}

export interface GeneratedImage {
  id: string;
  mimeType: string;
  base64: string;
  model: string;
}

export interface CloudImageProvider {
  id: 'openai-images';
  generate(request: GenerateImageRequest, apiKey: string): Promise<GeneratedImage>;
}

export function buildOpenAIImageRequestBody(
  request: GenerateImageRequest,
  model: string,
): Record<string, unknown> {
  return {
    model,
    prompt: request.prompt,
    size: request.size,
    response_format: 'b64_json',
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function parseOpenAIImageResponse(input: unknown, model: string): GeneratedImage {
  if (!isRecord(input) || !Array.isArray(input.data) || input.data.length === 0) {
    throw new Error('invalid image response: data');
  }
  const first = input.data[0];
  if (!isRecord(first) || typeof first.b64_json !== 'string' || first.b64_json.length === 0) {
    throw new Error('invalid image response: b64_json');
  }
  return {
    id: typeof input.id === 'string' ? input.id : 'synthetic-image',
    mimeType: 'image/png',
    base64: first.b64_json,
    model,
  };
}

export class MockOpenAIImageProvider implements CloudImageProvider {
  readonly id = 'openai-images';

  async generate(request: GenerateImageRequest, _apiKey: string): Promise<GeneratedImage> {
    const body = buildOpenAIImageRequestBody(request, 'synthetic-image-model');
    return {
      id: 'synthetic-image',
      mimeType: 'image/png',
      base64: Buffer.from(JSON.stringify(body)).toString('base64'),
      model: 'synthetic-image-model',
    };
  }
}
