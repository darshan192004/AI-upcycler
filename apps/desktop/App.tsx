import { useCallback, useState } from 'react';
import { Platform, Pressable, StyleSheet, Text, View } from 'react-native';

type Role = 'off' | 'master' | 'worker';

const ROLES: Role[] = ['off', 'master', 'worker'];
const COLORS: Record<Role, string> = {
  off: '#9CA3AF',
  master: '#5B8CFF',
  worker: '#34D399',
};

export default function App() {
  const [role, setRole] = useState<Role>('off');
  const advanceRole = useCallback(() => {
    setRole((current) => ROLES[(ROLES.indexOf(current) + 1) % ROLES.length] ?? 'off');
  }, []);

  return (
    <View style={styles.container}>
      <Text style={styles.title}>AI Upcycler Cluster</Text>
      <Text style={styles.subtitle}>{Platform.OS} device</Text>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Current role: ${role}. Tap to change role.`}
        onPress={advanceRole}
        style={({ pressed }) => [styles.roleButton, { backgroundColor: COLORS[role] }, pressed && styles.pressed]}
      >
        <Text style={styles.roleText}>{role}</Text>
      </Pressable>
      <Text style={styles.hint}>Pairing and layer placement arrive in the next tasks.</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: 'center',
    flex: 1,
    justifyContent: 'center',
    padding: 24,
  },
  hint: {
    color: '#6B7280',
    marginTop: 24,
    textAlign: 'center',
  },
  pressed: {
    opacity: 0.8,
  },
  roleButton: {
    alignItems: 'center',
    borderRadius: 12,
    marginTop: 32,
    minHeight: 56,
    justifyContent: 'center',
    minWidth: 200,
  },
  roleText: {
    color: '#FFFFFF',
    fontSize: 20,
    fontWeight: '600',
    textTransform: 'capitalize',
  },
  subtitle: {
    color: '#4B5563',
    fontSize: 16,
    marginTop: 8,
  },
  title: {
    fontSize: 28,
    fontWeight: '700',
    textAlign: 'center',
  },
});
