export interface CatalogEntry {
  repo_id: string;
  file_name: string;
  display_name: string;
  architecture: string;
  parameter_size: string;
  quantization: string;
  file_size_gb: number;
  min_ram_gb: number;
  layers: number;
}

export const HF_CATALOG: CatalogEntry[] = [
  {
    repo_id: 'Qwen/Qwen2.5-14B-Instruct-GGUF',
    file_name: 'qwen2.5-14b-instruct-q4_k_m.gguf',
    display_name: 'Qwen 2.5 14B Instruct',
    architecture: 'Qwen2',
    parameter_size: '14B',
    quantization: 'Q4_K_M',
    file_size_gb: 9.0,
    min_ram_gb: 10.5,
    layers: 48,
  },
  {
    repo_id: 'meta-llama/Llama-3.1-8B-Instruct-GGUF',
    file_name: 'llama-3.1-8b-instruct-q4_k_m.gguf',
    display_name: 'Llama 3.1 8B Instruct',
    architecture: 'Llama',
    parameter_size: '8B',
    quantization: 'Q4_K_M',
    file_size_gb: 4.9,
    min_ram_gb: 5.5,
    layers: 32,
  },
  {
    repo_id: 'bartowski/DeepSeek-R1-Distill-Qwen-14B-GGUF',
    file_name: 'deepseek-r1-distill-qwen-14b-q4_k_m.gguf',
    display_name: 'DeepSeek R1 Distill 14B',
    architecture: 'Qwen2',
    parameter_size: '14B',
    quantization: 'Q4_K_M',
    file_size_gb: 8.9,
    min_ram_gb: 10.0,
    layers: 48,
  },
  {
    repo_id: 'mistralai/Mistral-7B-Instruct-v0.3-GGUF',
    file_name: 'mistral-7b-instruct-v0.3-q5_k_m.gguf',
    display_name: 'Mistral 7B Instruct v0.3',
    architecture: 'Mistral',
    parameter_size: '7B',
    quantization: 'Q5_K_M',
    file_size_gb: 5.1,
    min_ram_gb: 5.8,
    layers: 32,
  },
  {
    repo_id: 'microsoft/Phi-3-mini-4k-instruct-gguf',
    file_name: 'phi-3-mini-4k-instruct-q8_0.gguf',
    display_name: 'Phi-3 Mini 3.8B',
    architecture: 'Phi3',
    parameter_size: '3.8B',
    quantization: 'Q8_0',
    file_size_gb: 4.1,
    min_ram_gb: 4.6,
    layers: 32,
  },
  {
    repo_id: 'Qwen/Qwen2.5-Coder-7B-Instruct-GGUF',
    file_name: 'qwen2.5-coder-7b-instruct-q4_k_m.gguf',
    display_name: 'Qwen 2.5 Coder 7B',
    architecture: 'Qwen2',
    parameter_size: '7B',
    quantization: 'Q4_K_M',
    file_size_gb: 4.7,
    min_ram_gb: 5.2,
    layers: 28,
  },
];
