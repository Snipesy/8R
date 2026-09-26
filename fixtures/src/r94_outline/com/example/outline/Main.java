package com.example.outline;

// Outlining (docs/sources/r8-desugar.md §4.5): StringBuilder chains repeated at >= 20 sites
// (classic outliner) and throw blocks (R8 9.x bottom-up throw outliner).
public class Main {
    static String fmt0(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(0).toString();
    }

    static int check0(int x) {
        if (x < -100) {
            throw new IllegalArgumentException("value " + x + " below -100");
        }
        return x + 0;
    }
    static String fmt1(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(1).toString();
    }

    static int check1(int x) {
        if (x < -99) {
            throw new IllegalArgumentException("value " + x + " below -99");
        }
        return x + 1;
    }
    static String fmt2(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(2).toString();
    }

    static int check2(int x) {
        if (x < -98) {
            throw new IllegalArgumentException("value " + x + " below -98");
        }
        return x + 2;
    }
    static String fmt3(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(3).toString();
    }

    static int check3(int x) {
        if (x < -97) {
            throw new IllegalArgumentException("value " + x + " below -97");
        }
        return x + 3;
    }
    static String fmt4(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(4).toString();
    }

    static int check4(int x) {
        if (x < -96) {
            throw new IllegalArgumentException("value " + x + " below -96");
        }
        return x + 4;
    }
    static String fmt5(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(5).toString();
    }

    static int check5(int x) {
        if (x < -95) {
            throw new IllegalArgumentException("value " + x + " below -95");
        }
        return x + 5;
    }
    static String fmt6(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(6).toString();
    }

    static int check6(int x) {
        if (x < -94) {
            throw new IllegalArgumentException("value " + x + " below -94");
        }
        return x + 6;
    }
    static String fmt7(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(7).toString();
    }

    static int check7(int x) {
        if (x < -93) {
            throw new IllegalArgumentException("value " + x + " below -93");
        }
        return x + 7;
    }
    static String fmt8(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(8).toString();
    }

    static int check8(int x) {
        if (x < -92) {
            throw new IllegalArgumentException("value " + x + " below -92");
        }
        return x + 8;
    }
    static String fmt9(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(9).toString();
    }

    static int check9(int x) {
        if (x < -91) {
            throw new IllegalArgumentException("value " + x + " below -91");
        }
        return x + 9;
    }
    static String fmt10(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(10).toString();
    }

    static int check10(int x) {
        if (x < -90) {
            throw new IllegalArgumentException("value " + x + " below -90");
        }
        return x + 10;
    }
    static String fmt11(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(11).toString();
    }

    static int check11(int x) {
        if (x < -89) {
            throw new IllegalArgumentException("value " + x + " below -89");
        }
        return x + 11;
    }
    static String fmt12(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(12).toString();
    }

    static int check12(int x) {
        if (x < -88) {
            throw new IllegalArgumentException("value " + x + " below -88");
        }
        return x + 12;
    }
    static String fmt13(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(13).toString();
    }

    static int check13(int x) {
        if (x < -87) {
            throw new IllegalArgumentException("value " + x + " below -87");
        }
        return x + 13;
    }
    static String fmt14(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(14).toString();
    }

    static int check14(int x) {
        if (x < -86) {
            throw new IllegalArgumentException("value " + x + " below -86");
        }
        return x + 14;
    }
    static String fmt15(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(15).toString();
    }

    static int check15(int x) {
        if (x < -85) {
            throw new IllegalArgumentException("value " + x + " below -85");
        }
        return x + 15;
    }
    static String fmt16(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(16).toString();
    }

    static int check16(int x) {
        if (x < -84) {
            throw new IllegalArgumentException("value " + x + " below -84");
        }
        return x + 16;
    }
    static String fmt17(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(17).toString();
    }

    static int check17(int x) {
        if (x < -83) {
            throw new IllegalArgumentException("value " + x + " below -83");
        }
        return x + 17;
    }
    static String fmt18(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(18).toString();
    }

    static int check18(int x) {
        if (x < -82) {
            throw new IllegalArgumentException("value " + x + " below -82");
        }
        return x + 18;
    }
    static String fmt19(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(19).toString();
    }

    static int check19(int x) {
        if (x < -81) {
            throw new IllegalArgumentException("value " + x + " below -81");
        }
        return x + 19;
    }
    static String fmt20(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(20).toString();
    }

    static int check20(int x) {
        if (x < -80) {
            throw new IllegalArgumentException("value " + x + " below -80");
        }
        return x + 20;
    }
    static String fmt21(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(21).toString();
    }

    static int check21(int x) {
        if (x < -79) {
            throw new IllegalArgumentException("value " + x + " below -79");
        }
        return x + 21;
    }
    static String fmt22(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(22).toString();
    }

    static int check22(int x) {
        if (x < -78) {
            throw new IllegalArgumentException("value " + x + " below -78");
        }
        return x + 22;
    }
    static String fmt23(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(23).toString();
    }

    static int check23(int x) {
        if (x < -77) {
            throw new IllegalArgumentException("value " + x + " below -77");
        }
        return x + 23;
    }
    static String fmt24(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(24).toString();
    }

    static int check24(int x) {
        if (x < -76) {
            throw new IllegalArgumentException("value " + x + " below -76");
        }
        return x + 24;
    }
    static String fmt25(String a, int b) {
        return new StringBuilder().append(a).append(':').append(b).append('#').append(25).toString();
    }

    static int check25(int x) {
        if (x < -75) {
            throw new IllegalArgumentException("value " + x + " below -75");
        }
        return x + 25;
    }

    public static void main(String[] args) {
        int n = args.length;
        String tag = "t" + args.length;
        StringBuilder sb = new StringBuilder();
        sb.append(fmt0(tag, n + 0)).append(check0(n)).append('\n');
        sb.append(fmt1(tag, n + 1)).append(check1(n)).append('\n');
        sb.append(fmt2(tag, n + 2)).append(check2(n)).append('\n');
        sb.append(fmt3(tag, n + 3)).append(check3(n)).append('\n');
        sb.append(fmt4(tag, n + 4)).append(check4(n)).append('\n');
        sb.append(fmt5(tag, n + 5)).append(check5(n)).append('\n');
        sb.append(fmt6(tag, n + 6)).append(check6(n)).append('\n');
        sb.append(fmt7(tag, n + 7)).append(check7(n)).append('\n');
        sb.append(fmt8(tag, n + 8)).append(check8(n)).append('\n');
        sb.append(fmt9(tag, n + 9)).append(check9(n)).append('\n');
        sb.append(fmt10(tag, n + 10)).append(check10(n)).append('\n');
        sb.append(fmt11(tag, n + 11)).append(check11(n)).append('\n');
        sb.append(fmt12(tag, n + 12)).append(check12(n)).append('\n');
        sb.append(fmt13(tag, n + 13)).append(check13(n)).append('\n');
        sb.append(fmt14(tag, n + 14)).append(check14(n)).append('\n');
        sb.append(fmt15(tag, n + 15)).append(check15(n)).append('\n');
        sb.append(fmt16(tag, n + 16)).append(check16(n)).append('\n');
        sb.append(fmt17(tag, n + 17)).append(check17(n)).append('\n');
        sb.append(fmt18(tag, n + 18)).append(check18(n)).append('\n');
        sb.append(fmt19(tag, n + 19)).append(check19(n)).append('\n');
        sb.append(fmt20(tag, n + 20)).append(check20(n)).append('\n');
        sb.append(fmt21(tag, n + 21)).append(check21(n)).append('\n');
        sb.append(fmt22(tag, n + 22)).append(check22(n)).append('\n');
        sb.append(fmt23(tag, n + 23)).append(check23(n)).append('\n');
        sb.append(fmt24(tag, n + 24)).append(check24(n)).append('\n');
        sb.append(fmt25(tag, n + 25)).append(check25(n)).append('\n');
        System.out.print(sb);
        try {
            check3(n - 1000);
        } catch (IllegalArgumentException e) {
            System.out.println("caught " + e.getMessage());
        }
    }
}
