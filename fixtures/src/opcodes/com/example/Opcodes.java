package com.example;

import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;

public class Opcodes {
    @Retention(RetentionPolicy.RUNTIME)
    public @interface Info {
        byte b() default -1;
        short s() default 300;
        char c() default 'x';
        int i() default -70000;
        long l() default 0x1122334455667788L;
        float f() default 1.5f;
        double d() default -2.25;
        boolean z() default true;
        String str() default "café 中 😀";
        Class<?> cls() default Opcodes.class;
        Thread.State e() default Thread.State.RUNNABLE;
        int[] arr() default {1, 2, 3};
    }

    public static final int CONST_INT = 123456;
    public static final long CONST_LONG = -1L;
    public static final String CONST_STR = "static value";
    public static final double CONST_DOUBLE = 3.5;
    static int counter;
    volatile long wide;
    private final Object lock = new Object();

    @Info
    public int arithmetic(int a, int b, long c, float d, double e) {
        int x = a + b; x -= a * b; x /= (b | 1); x %= 7; x &= 0xff; x |= 0x100; x ^= a;
        x <<= 2; x >>= 1; x >>>= 1;
        x += 5; x = 1000 - x; x *= 300; x = x / 12345; x = x % 3;
        long y = c * 3 + (c >> 2) - (c << 3) + (c >>> 4) + (c & 0xffffL) + (c | 7L) + (c ^ -1L);
        y = y / 3 + y % 5;
        float f = d * 2.0f + d / 3.0f - d % 1.5f;
        double g = e * f + e / y - e % 2.0;
        int n = -x + ~x;
        long m = -y + ~y;
        return (int) (n + m + (long) f + (long) g + (byte) x + (char) x + (short) x
                + (x < 0 ? 1 : 0) + (f > g ? 1 : 2) + (y == c ? 3 : 4) + (int) (float) m + (int) (double) f);
    }

    public int packedSwitch(int k) {
        switch (k) {
            case 1: return 10;
            case 2: return 20;
            case 3: return 30;
            case 4: return 40;
            case 5: return 55;
            default: return -1;
        }
    }

    public int sparseSwitch(int k) {
        switch (k) {
            case -1000: return 1;
            case 7: return 2;
            case 99999: return 3;
            case Integer.MAX_VALUE: return 4;
            default: return 0;
        }
    }

    public int stringSwitch(String s) {
        switch (s) {
            case "alpha": return 1;
            case "beta": return 2;
            case "gamma": return 3;
            default: return 0;
        }
    }

    public long arrays() {
        int[] ints = {1, 2, 3, 4, 5, 6, 7, 8};
        long[] longs = {1L << 40, -1L, 7L};
        byte[] bytes = {1, -2, 3};
        char[] chars = {'a', 'b'};
        short[] shorts = {-300, 300};
        boolean[] bools = new boolean[4];
        double[] doubles = {1.5, 2.5};
        String[] strs = {"a", "b", null};
        Object[][] multi = new Object[2][3];
        bools[1] = true;
        ints[2] = ints[3] + ints.length;
        bytes[0] = (byte) (bytes[1] + chars[0]);
        shorts[1] = (short) (shorts[0] + bools.length);
        multi[1][2] = strs[0];
        return ints[2] + longs[0] + bytes[0] + chars[1] + shorts[1] + (bools[1] ? 1 : 0)
                + (long) doubles[1] + multi.length + strs.length;
    }

    public String exceptions(Object o) {
        try {
            synchronized (lock) {
                if (o instanceof String) {
                    return ((String) o).trim();
                }
                throw new IllegalStateException("not a string: " + o);
            }
        } catch (IllegalStateException | ClassCastException ex) {
            return "caught " + ex.getMessage();
        } catch (Throwable t) {
            return "throwable";
        } finally {
            counter++;
        }
    }

    public long branches(int a, int b, long w) {
        long r = 0;
        if (a == b) r += 1;
        if (a != b) r += 2;
        if (a < b) r += 3;
        if (a >= b) r += 4;
        if (a > b) r += 5;
        if (a <= b) r += 6;
        if (a == 0) r += 7;
        if (a != 0) r += 8;
        if (a < 0) r += 9;
        if (a >= 0) r += 10;
        if (a > 0) r += 11;
        if (a <= 0) r += 12;
        for (int i = 0; i < b; i++) {
            if (i % 3 == 0) continue;
            if (i > 1000) break;
            r += i;
        }
        wide = w;
        return r + wide;
    }

    public Object constants() {
        long big = 0x123456789abcdefL;
        long high = 0x4010000000000000L;
        int highInt = 0x7fff0000;
        float fl = 1e10f;
        double db = 1e300;
        Class<?> c = int[].class;
        return big + high + highInt + (long) fl + (long) db + c.getName() + CONST_STR;
    }

    public int manyArgs(int a, long b, double c, Object d, int e, int f, long g, String h) {
        return staticMany(a, b, c, d, e, f, g, h) + staticMany(e, g, c, h, a, f, b, h);
    }

    static int staticMany(int a, long b, double c, Object d, int e, int f, long g, String h) {
        return (int) (a + b + c + (d == null ? 0 : 1) + e + f + g + h.length());
    }

    public Object methodHandles() throws Throwable {
        MethodHandle mh = MethodHandles.lookup().findStatic(Opcodes.class, "staticMany",
                MethodType.methodType(int.class, int.class, long.class, double.class, Object.class,
                        int.class, int.class, long.class, String.class));
        return (int) mh.invokeExact(1, 2L, 3.0, (Object) null, 5, 6, 7L, "eight");
    }

    public static void main(String[] args) throws Throwable {
        Opcodes o = new Opcodes();
        System.out.println(o.arithmetic(3, 4, 5L, 6f, 7.0));
        System.out.println(o.packedSwitch(3) + o.sparseSwitch(99999) + o.stringSwitch("beta"));
        System.out.println(o.arrays());
        System.out.println(o.exceptions("  x ") + o.exceptions(1));
        System.out.println(o.branches(1, 20, 3L));
        System.out.println(o.constants());
        System.out.println(o.manyArgs(1, 2L, 3.0, null, 5, 6, 7L, "eight"));
        System.out.println(o.methodHandles());
    }
}
