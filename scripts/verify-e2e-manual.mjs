const checklist = [
  'Windows master shows QR and Android worker joins by scan.',
  'Windows desktop worker joins Android master by OTP.',
  'GGUF split plan lists contributing devices.',
  'Chat/vision reply streams with contributor badges.',
  'Master generates one cloud image.',
  'Unpair revokes worker access.',
  'Evidence is stored under D: only.',
];

console.log('Two-device E2E acceptance checklist');
for (const item of checklist) {
  console.log(`- [ ] ${item}`);
}
console.log('Full runbook: docs/runbooks/two-device-e2e.md (added in Task 16).');
