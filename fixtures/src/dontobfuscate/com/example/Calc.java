package com.example;

// From the R8 rule audit (e18): with -dontobfuscate, removing the unused parameter of the
// second overload collides with the first, so R8 invents the fresh name `calc$1`.
public class Calc {
    static int calc(int a) {
        int r = a;
        for (int i = 0; i < a; i++) {
            r = r * 7 + i;
            if (r % 13 == 2) System.out.println("A" + r);
        }
        return r;
    }

    static int calc(int a, String unused) {
        int r = a * 3;
        for (int i = 0; i < a; i++) {
            r = r * 5 + i;
            if (r % 17 == 2) System.out.println("B" + r);
        }
        return r;
    }

    public static void run(String[] x) {
        System.out.println(calc(x.length) + calc(x.length + 1) + calc(x.length, "u") + calc(x.length + 2, "v"));
    }
}
